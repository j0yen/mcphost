//! PRD-mcphost-status-feed: the host's own per-minute self-probe, the
//! `/status.json` state computation, the daily rollup/prune, and the
//! alerting/auto-incident integration (requirement 6).
//!
//! Technical considerations: "the self samples call the same handler
//! functions the HTTP layer dispatches to, in process ... they must not
//! count in `calls` or metering". Every probe below is a direct in-process
//! call -- no HTTP round trip, no `calls` row, no metering event -- same
//! "tests never reach the network" shape this crate already gives
//! `AppState::billing_client`/`email_client`. `exec` goes through
//! `kinds::CallCtx` the same way `handler.rs`'s real dispatch does, under a
//! dedicated `status-feed-self-check` namespace so its `python` env never
//! collides with a real tenant's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::db::Incident;
use crate::errors::AppError;
use crate::state::{AppState, now_unix, rfc3339_from_unix};

/// requirement 2: the four fixed self-probed components, in the order
/// `/status.json` reports them.
pub const COMPONENTS: [&str; 4] = ["mcp", "exec", "billing", "claim"];

/// requirement 3: a component's state is drawn from its last 5 samples.
const STATE_WINDOW: i64 = 5;
/// requirement 6 / AC9: consecutive fails before `status.component_down`
/// fires and an auto-incident opens.
const AUTO_INCIDENT_OPEN_STREAK: usize = 5;
/// requirement 6 / AC9: consecutive ok samples before an auto-opened
/// incident auto-closes.
const AUTO_INCIDENT_CLOSE_STREAK: usize = 10;
/// requirement 1 / AC7: samples older than this are pruned daily.
pub const SAMPLE_RETENTION_DAYS: i64 = 90;
/// requirement 3: `uptime_30d`/`uptime_90d` windows.
const UPTIME_30D_DAYS: i64 = 30;
const UPTIME_90D_DAYS: i64 = 90;
/// requirement 3: `/status.json`'s `Cache-Control`.
pub const CACHE_MAX_AGE_SECS: u64 = 60;
/// requirement 3: `incidents_recent_30d`'s own window.
const RECENT_INCIDENTS_DAYS: i64 = 30;
/// The dedicated `python`-env namespace [`probe_exec`] calls under, distinct
/// from every real tenant namespace.
const SELF_CHECK_NAMESPACE: &str = "status-feed-self-check";
/// [`probe_exec`]/[`probe_mcp`]/[`probe_claim`]'s `CallCtx::tenant_id` --
/// `0` is never a real tenant id (`tenants.id` is an `AUTOINCREMENT` primary
/// key, starting at 1).
const SELF_CHECK_TENANT_ID: i64 = 0;
/// [`probe_claim`]'s probe token -- never a real claim token (those are
/// minted, high-entropy, hashed on lookup); a lookup miss is exactly the
/// "invalid token" path requirement 2 wants exercised.
const SELF_CHECK_CLAIM_TOKEN: &str = "status-feed-self-check-invalid-token";

/// Test-controllability for the four self-probes (same "field, not a real
/// subprocess/socket/env var, since tests run in parallel in one binary and
/// must not flake on real I/O" rule [`AppState::alert_config`]/
/// `sandbox_mechanism` already follow): when a component has an override
/// set, [`tick_once`] uses it instead of running the real probe below.
/// Empty in every real `mcphost serve` start.
#[derive(Clone, Default)]
pub struct ProbeOverrides(Arc<Mutex<HashMap<String, bool>>>);

impl ProbeOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, component: &str, ok: bool) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(component.to_string(), ok);
    }

    pub fn clear(&self, component: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(component);
    }

    fn get(&self, component: &str) -> Option<bool> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(component).copied()
    }
}

/// requirement 2: an in-process `initialize` + `tools/list` equivalent --
/// the database backing every response must be writable, and the same
/// anonymous tool listing `tools/list` builds
/// ([`crate::handler::self_check_tools_list`]) must come back non-empty.
async fn probe_mcp(state: &AppState) -> (bool, i64) {
    let start = Instant::now();
    let db_ok = state.db.is_writable().await;
    let tools_ok = crate::handler::self_check_tools_list(state) > 0;
    (db_ok && tools_ok, start.elapsed().as_millis() as i64)
}

/// requirement 2: an echo-kind call through the sandbox path -- absent
/// `python` kind registration (this crate's own signal that the sandbox
/// executor is unavailable, e.g. this PRD's own test harness default,
/// AC2) counts as failing outright, no call attempted.
async fn probe_exec(state: &AppState) -> (bool, i64) {
    let start = Instant::now();
    let Some(kind) = state.kinds.get("python") else {
        return (false, 0);
    };
    let spec = json!({"source": "def main(args):\n    return {\"ok\": True}\n"});
    let ctx = crate::kinds::CallCtx::for_test(SELF_CHECK_TENANT_ID, SELF_CHECK_NAMESPACE);
    let ok = kind.call(&spec, json!({}), &ctx).await.is_ok();
    (ok, start.elapsed().as_millis() as i64)
}

/// requirement 2: the billing webhook route answers a signed no-op --
/// `billing::process_webhook` is called in process with a synthetic,
/// correctly-signed, unrecognized event type (the same "ledgered, never
/// applied" path any real unknown Stripe event takes). Billing is optional
/// (goal 4 of PRD-grand-loop-billing): a host with no webhook secret
/// configured has nothing to probe here, so it reads as trivially ok
/// rather than a manufactured failure.
async fn probe_billing(state: &AppState) -> (bool, i64) {
    let start = Instant::now();
    let Some(secret) = state.billing_config.webhook_secret.clone() else {
        return (true, 0);
    };
    let ts = now_unix();
    let payload = json!({
        "id": format!("evt_status_self_check_{ts}"),
        "type": "status.self_check",
        "livemode": false,
        "data": {"object": {}},
    })
    .to_string();
    let signature = sign_stripe_payload(payload.as_bytes(), &secret, ts);
    let ok = crate::billing::process_webhook(state, payload.as_bytes(), &signature)
        .await
        .is_ok();
    (ok, start.elapsed().as_millis() as i64)
}

/// Same signing scheme [`crate::billing::verify_stripe_signature`] checks
/// (`t=<unix>,v1=<hmac-sha256 hex>` over `"<unix>.<body>"`), reimplemented
/// here (rather than exposed from `billing.rs`) since signing is otherwise
/// only ever something Stripe itself does -- this probe is the one
/// in-process exception.
fn sign_stripe_payload(payload: &[u8], secret: &str, ts: i64) -> String {
    let signed = [ts.to_string().as_bytes(), b".", payload].concat();
    let mac = crate::billing::hmac_sha256(secret.as_bytes(), &signed);
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    format!("t={ts},v1={hex}")
}

/// requirement 2: `/claim/<invalid-token>` -- calling
/// [`crate::claim::resolve_claim_token`] directly (the same lookup
/// `GET`/`POST /claim/{token}` dispatch to) is ok when it refuses the
/// token with anything other than a storage-error response; a resolved
/// tenant (a real token happening to collide, astronomically unlikely)
/// still counts as a probe failure worth surfacing.
async fn probe_claim(state: &AppState) -> (bool, i64) {
    let start = Instant::now();
    let ok = match crate::claim::resolve_claim_token(state, SELF_CHECK_CLAIM_TOKEN).await {
        Ok(_) => false,
        Err(resp) => resp.status() != StatusCode::INTERNAL_SERVER_ERROR,
    };
    (ok, start.elapsed().as_millis() as i64)
}

async fn run_probe(state: &AppState, component: &str) -> (bool, i64) {
    match component {
        "mcp" => probe_mcp(state).await,
        "exec" => probe_exec(state).await,
        "billing" => probe_billing(state).await,
        "claim" => probe_claim(state).await,
        _ => unreachable!("COMPONENTS is the only source of component names"),
    }
}

/// requirement 2: one minute-tick cycle -- probes (or reads the test
/// override for) every component in [`COMPONENTS`], records a sample, and
/// runs the alert/auto-incident check (requirement 6). Exposed directly
/// (not just via [`spawn_tick`]) so a test can drive a deterministic
/// number of ticks -- same convention as `alerts::tick_once`/
/// `bans::tick_once`.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let now = now_unix();
    for component in COMPONENTS {
        let (ok, latency_ms) = match state.status_probe_override.get(component) {
            Some(forced) => (forced, 0),
            None => run_probe(state, component).await,
        };
        state
            .db
            .insert_status_sample(component.to_string(), now, ok, latency_ms, "self".to_string())
            .await?;
        check_component_alert(state, component).await?;
    }
    Ok(())
}

/// requirement 6 / AC9: fires `status.component_down` and auto-opens a
/// `partial`-impact incident the first tick a component's trailing samples
/// hit [`AUTO_INCIDENT_OPEN_STREAK`] consecutive fails, provided no open
/// incident already names it (an operator-opened one included, so this
/// never steps on a human's own incident). Auto-closes (with a timeline
/// note) any incident *this mechanism* opened once the trailing samples
/// hit [`AUTO_INCIDENT_CLOSE_STREAK`] consecutive ok.
async fn check_component_alert(state: &AppState, component: &str) -> Result<(), AppError> {
    let recent = state
        .db
        .recent_status_samples(component.to_string(), AUTO_INCIDENT_CLOSE_STREAK as i64)
        .await?;
    let fail_streak = recent.iter().take_while(|s| !s.ok).count();
    let ok_streak = recent.iter().take_while(|s| s.ok).count();

    if fail_streak >= AUTO_INCIDENT_OPEN_STREAK {
        let already_named = state
            .db
            .list_open_incidents()
            .await?
            .iter()
            .any(|i| incident_names_component(i, component));
        if !already_named {
            open_auto_incident(state, component).await?;
        }
    } else if ok_streak >= AUTO_INCIDENT_CLOSE_STREAK {
        for incident in state.db.list_open_incidents().await? {
            if incident.auto && incident_names_component(&incident, component) {
                close_auto_incident(state, &incident).await?;
            }
        }
    }
    Ok(())
}

fn incident_names_component(incident: &Incident, component: &str) -> bool {
    serde_json::from_str::<Vec<String>>(&incident.components_json)
        .map(|names| names.iter().any(|n| n == component))
        .unwrap_or(false)
}

async fn open_auto_incident(state: &AppState, component: &str) -> Result<(), AppError> {
    let now = now_unix();
    let title = format!("{component} is failing self-checks");
    let timeline = json!([{"ts": now, "type": "opened", "message": title}]).to_string();
    let components_json = json!([component]).to_string();
    let id = state
        .db
        .insert_incident(title, "partial".to_string(), components_json, now, timeline, true)
        .await?;
    crate::alerts::raise(
        state,
        crate::alerts::RaiseInput {
            key: "status.component_down".to_string(),
            severity: crate::alerts::Severity::Warn,
            title: format!("{component} is down"),
            body: json!({"component": component, "incident_id": id}),
        },
    )
    .await?;
    Ok(())
}

async fn close_auto_incident(state: &AppState, incident: &Incident) -> Result<(), AppError> {
    let now = now_unix();
    let entry = json!({
        "ts": now,
        "type": "closed",
        "message": format!("auto-resolved: {AUTO_INCIDENT_CLOSE_STREAK} consecutive ok samples"),
    });
    state.db.incident_close(incident.id, entry, now).await?;
    Ok(())
}

/// requirement 2: the per-minute self-sampler, started once alongside the
/// other background tasks in `main.rs`.
pub fn spawn_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "status feed self-sample tick failed");
            }
        }
    })
}

/// requirement 3: `"ok"` when every one of a component's last
/// [`STATE_WINDOW`] samples (or fewer, if it hasn't been sampled that many
/// times yet) is ok; `"failing"` otherwise, including "never sampled" --
/// binary, per requirement 3's own language ("no failing component" /
/// "one failing component"), same two-state model AC2 tests directly.
fn component_state(samples: &[crate::db::StatusSample]) -> &'static str {
    if samples.is_empty() {
        return "failing";
    }
    let window = &samples[..samples.len().min(STATE_WINDOW as usize)];
    if window.iter().all(|s| s.ok) {
        "ok"
    } else {
        "failing"
    }
}

/// requirement 3: `outage` for a `major`-impact open incident or every
/// component failing; `degraded` for a `partial`-impact open incident or
/// one failing component; `operational` otherwise.
fn overall_state(component_states: &[&str], open_incidents: &[Incident]) -> &'static str {
    let all_failing = !component_states.is_empty() && component_states.iter().all(|s| *s == "failing");
    let has_major = open_incidents.iter().any(|i| i.impact == "major");
    if has_major || all_failing {
        return "outage";
    }
    let has_partial = open_incidents.iter().any(|i| i.impact == "partial");
    let any_failing = component_states.contains(&"failing");
    if has_partial || any_failing {
        return "degraded";
    }
    "operational"
}

/// `pub(crate)` -- `admin.rs`'s `incident_open`/`incident_update`/
/// `incident_close` reuse this for the same wire shape `/status.json`'s
/// `incidents_open`/`incidents_recent_30d` already use.
pub(crate) fn incident_json(incident: &Incident) -> Value {
    json!({
        "id": incident.id,
        "title": incident.title,
        "impact": incident.impact,
        "components": serde_json::from_str::<Value>(&incident.components_json).unwrap_or_else(|_| json!([])),
        "opened_at": incident.opened_at,
        "closed_at": incident.closed_at,
        "timeline": serde_json::from_str::<Value>(&incident.timeline_json).unwrap_or_else(|_| json!([])),
    })
}

/// requirement 3/AC1/AC5: `uptime_30d`/`uptime_90d` -- from the daily
/// rollup when at least one rollup row falls in the window, otherwise from
/// raw samples directly (a freshly-started host, AC1, has samples long
/// before its first rollup run).
async fn uptime_for_window(state: &AppState, component: &str, days: i64) -> Result<Option<f64>, AppError> {
    let since_day = rfc3339_from_unix(now_unix() - days * 86_400)[..10].to_string();
    let (ok, total) = state
        .db
        .status_daily_sum_since(component.to_string(), since_day)
        .await?;
    if total > 0 {
        return Ok(Some(ok as f64 / total as f64));
    }
    let since_ts = now_unix() - days * 86_400;
    let (ok, total) = state
        .db
        .status_samples_ok_ratio_since(component.to_string(), since_ts)
        .await?;
    if total > 0 {
        Ok(Some(ok as f64 / total as f64))
    } else {
        Ok(None)
    }
}

/// requirement 3: `GET /status.json`'s whole body.
pub async fn status_json(state: &AppState) -> Result<Value, AppError> {
    let mut components = Vec::with_capacity(COMPONENTS.len());
    let mut states = Vec::with_capacity(COMPONENTS.len());
    for component in COMPONENTS {
        let recent = state
            .db
            .recent_status_samples(component.to_string(), STATE_WINDOW)
            .await?;
        let comp_state = component_state(&recent);
        states.push(comp_state);
        let last_sample = recent.first().map(|s| {
            json!({"ts": s.ts, "ok": s.ok, "latency_ms": s.latency_ms, "source": s.source})
        });
        let uptime_30d = uptime_for_window(state, component, UPTIME_30D_DAYS).await?;
        let uptime_90d = uptime_for_window(state, component, UPTIME_90D_DAYS).await?;
        components.push(json!({
            "name": component,
            "state": comp_state,
            "uptime_30d": uptime_30d,
            "uptime_90d": uptime_90d,
            "last_sample": last_sample,
        }));
    }
    let open_incidents = state.db.list_open_incidents().await?;
    let recent_closed = state
        .db
        .list_recent_closed_incidents(now_unix() - RECENT_INCIDENTS_DAYS * 86_400)
        .await?;
    let overall = overall_state(&states, &open_incidents);
    Ok(json!({
        "state": overall,
        "generated_at": now_unix(),
        "components": components,
        "incidents_open": open_incidents.iter().map(incident_json).collect::<Vec<_>>(),
        "incidents_recent_30d": recent_closed.iter().map(incident_json).collect::<Vec<_>>(),
    }))
}

/// AC10: `GET /status.json?component=<name>&days=<n>` -- the daily rows
/// `admin.status.rollup`/the daily tick already populate.
pub async fn daily_rows(state: &AppState, component: &str, days: i64) -> Result<Value, AppError> {
    let rows = state.db.status_daily_recent(component.to_string(), days).await?;
    Ok(json!({
        "component": component,
        "days": rows.iter().map(|r| json!({
            "day": r.day,
            "ok_samples": r.ok_samples,
            "total_samples": r.total_samples,
            "p95_latency_ms": r.p95_latency_ms,
        })).collect::<Vec<_>>(),
    }))
}

/// requirement 1/AC5: `p95` of `latencies` -- nearest-rank method (the 95th
/// percentile is the `ceil(0.95 * n)`-th smallest value), the same
/// definition `docs/benchmarks/ac11-load-smoke.txt`'s own `p95` line uses.
fn p95_latency_ms(latencies: &[i64]) -> i64 {
    if latencies.is_empty() {
        return 0;
    }
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    let rank = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

/// `"YYYY-MM-DD"` -> `[start, end)` unix-second bounds for that UTC
/// calendar day, via the same `days_from_civil` calendar math `cron.rs`
/// already uses -- no separate calendar table needed for a leap day to
/// roll up correctly.
fn day_bounds(day: &str) -> Result<(i64, i64), AppError> {
    let bad_day = || AppError::Internal(format!("status feed: not a YYYY-MM-DD day: '{day}'"));
    let parts: Vec<&str> = day.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        return Err(bad_day());
    };
    let y: i64 = y.parse().map_err(|_| bad_day())?;
    let m: u32 = m.parse().map_err(|_| bad_day())?;
    let d: u32 = d.parse().map_err(|_| bad_day())?;
    let start = crate::state::days_from_civil(y, m, d) * 86_400;
    Ok((start, start + 86_400))
}

/// requirement 1/AC5: recomputes `status_daily` for one `(component,
/// day)` from every raw sample stored for that UTC calendar day. A no-op
/// (never writes a row) when there is no sample for that day -- a rollup
/// never invents a zero-sample day.
pub async fn rollup_day(state: &AppState, component: &str, day: &str) -> Result<(), AppError> {
    let (start, end) = day_bounds(day)?;
    let samples = state
        .db
        .status_samples_for_day(component.to_string(), start, end)
        .await?;
    if samples.is_empty() {
        return Ok(());
    }
    let ok_samples = samples.iter().filter(|s| s.ok).count() as i64;
    let total_samples = samples.len() as i64;
    let p95 = p95_latency_ms(&samples.iter().map(|s| s.latency_ms).collect::<Vec<_>>());
    state
        .db
        .upsert_status_daily(component.to_string(), day.to_string(), ok_samples, total_samples, p95)
        .await
}

/// requirement 1/8: recomputes every `(component, day)` pair with at least
/// one raw sample in `[since_ts, until_ts)` -- the daily tick's own range
/// (yesterday and today) and `admin.status.rollup`'s caller-chosen range
/// both funnel through here.
pub async fn rollup_range(state: &AppState, since_ts: i64, until_ts: i64) -> Result<i64, AppError> {
    let pairs = state.db.status_sample_days_in_range(since_ts, until_ts).await?;
    let n = pairs.len() as i64;
    for (component, day) in pairs {
        rollup_day(state, &component, &day).await?;
    }
    Ok(n)
}

/// requirement 1/AC7: deletes raw samples older than
/// [`SAMPLE_RETENTION_DAYS`]; `status_daily` rows are never touched --
/// uptime history survives the raw-sample prune.
pub async fn prune_once(state: &AppState) -> Result<i64, AppError> {
    let cutoff = now_unix() - SAMPLE_RETENTION_DAYS * 86_400;
    state.db.prune_status_samples_older_than(cutoff).await
}

/// requirement 1: one daily cycle -- rolls up yesterday and today (today's
/// row is necessarily partial; recomputing it daily keeps it current
/// without a separate "close out the day" trigger) then prunes. Exposed
/// directly (not just via [`spawn_daily_tick`]) so a test can drive one
/// deterministic cycle -- same convention as [`tick_once`].
pub async fn daily_tick_once(state: &AppState) -> Result<(), AppError> {
    let now = now_unix();
    rollup_range(state, now - 2 * 86_400, now).await?;
    prune_once(state).await?;
    Ok(())
}

const DAILY_TICK_HOUR_UTC: i64 = 4;
const DAILY_TICK_MINUTE_UTC: i64 = 15;

fn next_daily_tick_unix(now_unix: i64) -> i64 {
    let days = now_unix.div_euclid(86_400);
    let today_run = days * 86_400 + DAILY_TICK_HOUR_UTC * 3600 + DAILY_TICK_MINUTE_UTC * 60;
    if today_run > now_unix {
        today_run
    } else {
        today_run + 86_400
    }
}

fn duration_until_next_daily_tick(now_unix: i64) -> Duration {
    Duration::from_secs((next_daily_tick_unix(now_unix) - now_unix).max(0) as u64)
}

/// requirement 1: the daily rollup+prune cycle, started once alongside the
/// other background tasks in `main.rs`.
pub fn spawn_daily_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    spawn_daily_loop(state, None)
}

/// Test-only: same loop [`spawn_daily_tick`] runs at real `serve` startup,
/// but firing every `interval` instead of waiting out the real ~04:15 UTC
/// cadence -- same "swap a short interval in for the test" convention as
/// `retention::spawn_prune_scheduler_for_test`.
pub fn spawn_daily_tick_for_test(state: AppState, interval: Duration) -> tokio::task::JoinHandle<()> {
    spawn_daily_loop(state, Some(interval))
}

fn spawn_daily_loop(state: AppState, interval_override: Option<Duration>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let sleep_for =
                interval_override.unwrap_or_else(|| duration_until_next_daily_tick(now_unix()));
            tokio::time::sleep(sleep_for).await;
            if let Err(e) = daily_tick_once(&state).await {
                tracing::warn!(error = %e, "status feed daily tick failed");
            }
        }
    })
}
