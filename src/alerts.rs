//! PRD-mcphost-alerting-webhook: alert sources, cooldown-collapsed storage,
//! and delivery (a webhook POST and/or the operator tenant's own inbox).
//! Raising an alert (`raise`) is synchronous only for its own `alerts` row
//! insert (non-functional requirement: never blocks the request path more
//! than 5ms); delivery always runs as a background task.
//!
//! Also carries PRD-mcphost-sqlite-busy-timeout-audit requirement 6 (P1,
//! AC9)'s own minimal, in-process alerting source registry --
//! [`AlertRegistry`]/[`ContentionTracker`] predate this module's webhook/
//! inbox pipeline above and stay independent of it: `db.contention` raises
//! through the in-memory [`AlertRegistry`] (AC9's own test reads
//! [`AlertRegistry::events`] directly), not through the persisted `alerts`
//! table [`raise`] writes to.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 4's default, absent `$MCPHOST_ALERT_COOLDOWN_SECS`.
pub const COOLDOWN_SECS_DEFAULT: i64 = 900;
/// requirement 7's default, absent `$MCPHOST_ALERT_MIN_SEVERITY`.
pub const MIN_SEVERITY_DEFAULT: Severity = Severity::Warn;
/// requirement 3: the webhook POST's own per-attempt deadline.
pub const WEBHOOK_TIMEOUT_SECS: u64 = 5;
/// requirement 3: the documented backoff between the initial webhook
/// attempt and each of its 3 retries. A field on [`AlertConfig`] (not just
/// this constant) so a test can shrink it without a real ~31s wait for the
/// full retry sequence to exhaust -- same "field, not just a constant"
/// convention [`crate::state::AppState::claim_token_ttl_secs`] already
/// establishes.
pub const RETRY_BACKOFF_MS_DEFAULT: [u64; 3] = [1_000, 5_000, 25_000];
/// technical considerations: the pause-file watcher's own poll cadence --
/// comfortably inside AC1's 30s "operator notified" bound.
pub const PAUSE_POLL_INTERVAL_SECS: u64 = 3;
/// requirement 2 / AC3's default, absent
/// `$MCPHOST_ALERT_QUOTA_TRIP_THRESHOLD`: the same plan knob tripping this
/// many times in [`QUOTA_TRIP_WINDOW_SECS`] raises one `quota.trip` alert.
pub const QUOTA_TRIP_THRESHOLD_DEFAULT: usize = 20;
pub const QUOTA_TRIP_WINDOW_SECS: u64 = 300;
/// requirement 2 / AC4's defaults, absent
/// `$MCPHOST_ALERT_ERROR_RATE_PCT`/`$MCPHOST_ALERT_ERROR_RATE_MIN_CALLS`.
pub const ERROR_RATE_PCT_DEFAULT: f64 = 5.0;
pub const ERROR_RATE_MIN_CALLS_DEFAULT: i64 = 50;
pub const ERROR_RATE_WINDOW_SECS: i64 = 300;
/// requirement 5 / AC11: `admin.alerts.raise`'s own body size ceiling --
/// same "field a plan/env can override, code enforces the constant"
/// convention [`crate::state::MAX_SPEC_BYTES`] already uses.
pub const MAX_ALERT_BODY_BYTES: usize = 16 * 1024;

/// requirement 7: `info` < `warn` < `critical` -- declaration order is
/// `Severity`'s derived `Ord`, so `MCPHOST_ALERT_MIN_SEVERITY: critical`
/// filtering out a `warn` alert is exactly `Severity::Warn < Severity::Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Critical => "critical",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "info" => Some(Severity::Info),
            "warn" => Some(Severity::Warn),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }
}

/// The env-derived settings every alert source/delivery path reads through
/// -- loaded once at startup ([`AlertConfig::from_env`]), same "load once,
/// hand every request a shared read-only view" convention
/// [`crate::state::AppState::plans`] already uses.
#[derive(Clone)]
pub struct AlertConfig {
    /// requirement 3: `None` disables webhook delivery (stored only).
    pub webhook_url: Option<String>,
    /// requirement 3: the operator tenant's namespace; `None` disables
    /// inbox delivery.
    pub tenant: Option<String>,
    pub cooldown_secs: i64,
    pub min_severity: Severity,
    pub retry_backoff_ms: [u64; 3],
    pub quota_trip_threshold: usize,
    pub error_rate_pct: f64,
    pub error_rate_min_calls: i64,
}

impl AlertConfig {
    pub fn from_env() -> Self {
        let webhook_url = std::env::var("MCPHOST_ALERT_WEBHOOK_URL")
            .ok()
            .filter(|s| !s.is_empty());
        let tenant = std::env::var("MCPHOST_ALERT_TENANT").ok().filter(|s| !s.is_empty());
        let cooldown_secs = std::env::var("MCPHOST_ALERT_COOLDOWN_SECS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(COOLDOWN_SECS_DEFAULT);
        let min_severity = std::env::var("MCPHOST_ALERT_MIN_SEVERITY")
            .ok()
            .and_then(|v| Severity::parse(&v))
            .unwrap_or(MIN_SEVERITY_DEFAULT);
        let retry_backoff_ms = std::env::var("MCPHOST_ALERT_RETRY_BACKOFF_MS")
            .ok()
            .and_then(|v| parse_backoff_ms(&v))
            .unwrap_or(RETRY_BACKOFF_MS_DEFAULT);
        let quota_trip_threshold = std::env::var("MCPHOST_ALERT_QUOTA_TRIP_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(QUOTA_TRIP_THRESHOLD_DEFAULT);
        let error_rate_pct = std::env::var("MCPHOST_ALERT_ERROR_RATE_PCT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(ERROR_RATE_PCT_DEFAULT);
        let error_rate_min_calls = std::env::var("MCPHOST_ALERT_ERROR_RATE_MIN_CALLS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(ERROR_RATE_MIN_CALLS_DEFAULT);
        Self {
            webhook_url,
            tenant,
            cooldown_secs,
            min_severity,
            retry_backoff_ms,
            quota_trip_threshold,
            error_rate_pct,
            error_rate_min_calls,
        }
    }

    /// requirement 6 / AC8, AC10: `/healthz`'s `alerts.sink` -- purely a
    /// function of which sinks are configured, independent of
    /// `min_severity` filtering (a `warn` alert filtered out by
    /// `MCPHOST_ALERT_MIN_SEVERITY: critical` still reports `sink: "webhook"`
    /// if a webhook URL is set; AC9 is about that one alert's own
    /// `delivery_status`, not this host-wide descriptor).
    pub fn sink_label(&self) -> &'static str {
        match (self.webhook_url.is_some(), self.tenant.is_some()) {
            (true, true) => "both",
            (true, false) => "webhook",
            (false, true) => "inbox",
            (false, false) => "store-only",
        }
    }
}

impl Default for AlertConfig {
    /// The "no env vars set" state -- both sinks unset (AC8's store-only),
    /// every threshold at its documented default. Every test server that
    /// doesn't care about alerting builds this rather than reading process
    /// env (test binaries run many tests in parallel; env is shared and
    /// racy -- same rationale `TestServer::start_with_signup_rate_limit`'s
    /// own doc comment states for that field).
    fn default() -> Self {
        Self {
            webhook_url: None,
            tenant: None,
            cooldown_secs: COOLDOWN_SECS_DEFAULT,
            min_severity: MIN_SEVERITY_DEFAULT,
            retry_backoff_ms: RETRY_BACKOFF_MS_DEFAULT,
            quota_trip_threshold: QUOTA_TRIP_THRESHOLD_DEFAULT,
            error_rate_pct: ERROR_RATE_PCT_DEFAULT,
            error_rate_min_calls: ERROR_RATE_MIN_CALLS_DEFAULT,
        }
    }
}

fn parse_backoff_ms(raw: &str) -> Option<[u64; 3]> {
    let parts = raw
        .split(',')
        .map(|s| s.trim().parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() == 3 {
        Some([parts[0], parts[1], parts[2]])
    } else {
        None
    }
}

/// [`raise`]'s own input -- one alert source's stable `key`, severity,
/// human title, and structured detail (echoed verbatim into the webhook
/// POST and the inbox notice's `data.body`).
pub struct RaiseInput {
    pub key: String,
    pub severity: Severity,
    pub title: String,
    pub body: Value,
}

struct RaisedAlert {
    id: i64,
    key: String,
    severity: Severity,
    title: String,
    body: Value,
    raised_at: i64,
}

/// requirement 4: raises `input` -- collapses into the open row (increments
/// `repeat_count`, no delivery) when the most recent row for `input.key` is
/// still within `MCPHOST_ALERT_COOLDOWN_SECS`; otherwise inserts a fresh
/// row (carrying the collapsed row's own `repeat_count` in
/// `body.previous_repeat_count`, when there was one) and spawns delivery in
/// the background. Returns the id of the row this raise landed on (the
/// existing open one, or the freshly inserted one).
pub async fn raise(state: &AppState, input: RaiseInput) -> Result<i64, AppError> {
    let now = now_unix();
    let recent = state.db.most_recent_alert_for_key(input.key.clone()).await?;
    if let Some(row) = &recent
        && now - row.raised_at < state.alert_config.cooldown_secs
    {
        state.db.increment_alert_repeat(row.id).await?;
        return Ok(row.id);
    }

    let mut body = input.body;
    if let Some(prev) = &recent
        && let Value::Object(map) = &mut body
    {
        map.insert("previous_repeat_count".to_string(), json!(prev.repeat_count));
    }
    let id = state
        .db
        .insert_alert(
            input.key.clone(),
            input.severity.as_str().to_string(),
            input.title.clone(),
            body.to_string(),
            now,
        )
        .await?;

    let deliver_state = state.clone();
    let alert = RaisedAlert {
        id,
        key: input.key,
        severity: input.severity,
        title: input.title,
        body,
        raised_at: now,
    };
    tokio::spawn(async move {
        deliver(&deliver_state, alert).await;
    });
    Ok(id)
}

/// The background delivery task [`raise`] spawns: decides `store-only` /
/// `skipped` / `delivered` / `failed` and writes it back.
async fn deliver(state: &AppState, alert: RaisedAlert) {
    let cfg = &state.alert_config;
    if cfg.webhook_url.is_none() && cfg.tenant.is_none() {
        // requirement 3 / AC8: neither sink configured -- stored only.
        let _ = state.db.update_alert_delivery(alert.id, "store-only", None).await;
        return;
    }
    if alert.severity < cfg.min_severity {
        // requirement 7 / AC9: filters delivery, never storage.
        let _ = state.db.update_alert_delivery(alert.id, "skipped", None).await;
        return;
    }

    let payload = json!({
        "id": alert.id,
        "key": alert.key,
        "severity": alert.severity.as_str(),
        "title": alert.title,
        "body": alert.body,
        "raised_at": alert.raised_at,
        "host": state.public_url,
    });

    let webhook_delivered = match &cfg.webhook_url {
        Some(url) => Some(deliver_webhook(state, url, &payload, &cfg.retry_backoff_ms).await),
        None => None,
    };
    let inbox_delivered = match &cfg.tenant {
        Some(tenant_ns) => Some(deliver_inbox(state, tenant_ns, &alert).await),
        None => None,
    };

    // AC5: when a webhook is configured, `delivery_status` tracks its own
    // outcome (an inbox send alongside it is best-effort, not what this
    // status names). Absent a webhook, it tracks the inbox send instead.
    let delivered = webhook_delivered.or(inbox_delivered).unwrap_or(false);
    let status: &'static str = if delivered { "delivered" } else { "failed" };
    let delivered_at = delivered.then(now_unix);
    let _ = state.db.update_alert_delivery(alert.id, status, delivered_at).await;
}

/// requirement 3 / AC1, AC5: one POST plus up to 3 retries at the
/// documented backoff (1s, 5s, 25s by default); `true` on the first `2xx`
/// response, `false` once every attempt has failed (a non-2xx status, a
/// timeout, or a connection error all count as a failed attempt -- the
/// host never distinguishes them here, only `delivery_status` downstream
/// cares).
async fn deliver_webhook(state: &AppState, url: &str, payload: &Value, backoff_ms: &[u64; 3]) -> bool {
    for (attempt, delay_ms) in std::iter::once(0u64).chain(backoff_ms.iter().copied()).enumerate() {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        let sent = state
            .http_client
            .post(url)
            .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
            .json(payload)
            .send()
            .await;
        if let Ok(resp) = sent
            && resp.status().is_success()
        {
            return true;
        }
    }
    false
}

/// requirement 3 / AC6: writes the "system" inbox notice via
/// [`crate::db::Db::insert_alert_notice`]; `false` (logged, never panics)
/// when the configured `$MCPHOST_ALERT_TENANT` doesn't resolve to a tenant,
/// or the write itself fails.
async fn deliver_inbox(state: &AppState, tenant_ns: &str, alert: &RaisedAlert) -> bool {
    let tenant = match state.db.find_tenant_by_namespace(tenant_ns.to_string()).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            tracing::warn!(tenant = tenant_ns, "alert inbox delivery: tenant not found");
            return false;
        }
        Err(e) => {
            tracing::warn!(error = %e, tenant = tenant_ns, "alert inbox delivery: tenant lookup failed");
            return false;
        }
    };
    match state
        .db
        .insert_alert_notice(
            tenant.id,
            alert.id,
            alert.key.clone(),
            alert.severity.as_str().to_string(),
            alert.title.clone(),
        )
        .await
    {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "alert inbox delivery failed");
            false
        }
    }
}

/// requirement 2 / AC3: the in-memory sliding-window counter behind
/// `quota.trip` -- same shape/rationale as
/// [`crate::state::ToolRunLimiter`], keyed by `(tenant_id, knob)` instead
/// of just `tenant_id`, and reporting the post-insert window count back to
/// the caller (rather than a plain admit/reject bool) since the caller's
/// own threshold check is the point of tracking this at all.
/// `(tenant_id, knob)` -> its recent trip timestamps -- factored into a
/// named alias so [`QuotaTripTracker`]'s field declaration doesn't trip
/// clippy::type_complexity, same convention `crate::bans::BanMap` uses.
type QuotaTripWindows = HashMap<(i64, String), VecDeque<Instant>>;

#[derive(Clone, Default)]
pub struct QuotaTripTracker {
    windows: Arc<Mutex<QuotaTripWindows>>,
}

impl QuotaTripTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one trip for `(tenant_id, knob)` and returns how many trips
    /// that same pair has accumulated within [`QUOTA_TRIP_WINDOW_SECS`],
    /// this one included.
    pub fn record(&self, tenant_id: i64, knob: &str) -> usize {
        let now = Instant::now();
        let mut guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.entry((tenant_id, knob.to_string())).or_default();
        window.push_back(now);
        while let Some(&oldest) = window.front() {
            if now.duration_since(oldest) >= Duration::from_secs(QUOTA_TRIP_WINDOW_SECS) {
                window.pop_front();
            } else {
                break;
            }
        }
        window.len()
    }
}

/// requirement 2 / AC4: one minute-tick cycle for the `errors.rate` source
/// -- raises when the trailing [`ERROR_RATE_WINDOW_SECS`] (5 min) has at
/// least `MCPHOST_ALERT_ERROR_RATE_MIN_CALLS` (default 50) calls AND more
/// than `MCPHOST_ALERT_ERROR_RATE_PCT` percent (default 5%) of them failed.
/// Exposed directly (not just via [`spawn_error_rate_tick`]) so a test can
/// drive one deterministic cycle instead of waiting on the real 60s
/// cadence -- same convention as `crate::bans::tick_once`.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let now = now_unix();
    let since = now - ERROR_RATE_WINDOW_SECS;
    let (total, errors) = state.db.calls_error_stats_since(since).await?;
    if total < state.alert_config.error_rate_min_calls {
        return Ok(());
    }
    let rate = errors as f64 / total as f64 * 100.0;
    if rate > state.alert_config.error_rate_pct {
        raise(
            state,
            RaiseInput {
                key: "errors.rate".to_string(),
                severity: Severity::Warn,
                title: format!("error rate {rate:.1}% over the last 5 minutes"),
                body: json!({
                    "rate_pct": rate,
                    "errors": errors,
                    "total": total,
                    "window_secs": ERROR_RATE_WINDOW_SECS,
                }),
            },
        )
        .await?;
    }
    Ok(())
}

/// requirement 2: the `errors.rate` minute tick, started once alongside
/// the other background tasks in `main.rs`.
pub fn spawn_error_rate_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "errors.rate tick failed");
            }
        }
    })
}

/// requirement 2 (`signup.paused`/`signup.resumed`), technical
/// considerations: "hook raise/resume at the transition, not on every
/// refused signup". The pause file is operator-edited on disk, with no
/// in-process setter to hook -- so the transition is detected by polling
/// [`crate::state::SignupPause::status`] every [`PAUSE_POLL_INTERVAL_SECS`]
/// and comparing against the previous poll, exactly the "raise once, on
/// the flip" the requirement asks for. The first poll only seeds `last`
/// (an already-paused host at startup does not itself raise an alert).
pub fn spawn_pause_watch(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last: Option<bool> = None;
        loop {
            let paused = state.signup_pause.status().is_some();
            if let Some(prev) = last
                && prev != paused
            {
                let (key, title) = if paused {
                    ("signup.paused", "signups are paused")
                } else {
                    ("signup.resumed", "signups have resumed")
                };
                if let Err(e) = raise(
                    &state,
                    RaiseInput {
                        key: key.to_string(),
                        severity: Severity::Warn,
                        title: title.to_string(),
                        body: json!({}),
                    },
                )
                .await
                {
                    tracing::warn!(error = %e, key, "failed to raise pause-transition alert");
                }
            }
            last = Some(paused);
            tokio::time::sleep(Duration::from_secs(PAUSE_POLL_INTERVAL_SECS)).await;
        }
    })
}

/// PRD-mcphost-sqlite-busy-timeout-audit requirement 6 (P1, AC9): a
/// minimal, in-process alerting source registry -- "the alerting source
/// registry is present" is unconditionally true here (this crate's own
/// registry; "no hard dependency" means no *external* one, not that this
/// PRD builds nothing), and [`ContentionTracker`] decides when
/// `db_busy_total`'s rise over a trailing window earns a `db.contention`
/// alert.
#[derive(Debug, Clone)]
pub struct AlertEvent {
    pub key: &'static str,
    pub detail: Value,
    pub raised_unix: i64,
}

/// Every alert this process has ever raised, in memory -- `admin.*` has no
/// reader for this yet (out of this PRD's scope beyond AC9's own test,
/// which reads [`AlertRegistry::events`] directly); logged at `warn` on
/// every raise so it's visible without one.
#[derive(Clone, Default)]
pub struct AlertRegistry {
    events: Arc<Mutex<Vec<AlertEvent>>>,
}

impl AlertRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raise(&self, key: &'static str, detail: Value) {
        let event = AlertEvent {
            key,
            detail: detail.clone(),
            raised_unix: crate::state::now_unix(),
        };
        tracing::warn!(alert_key = key, detail = %detail, "alert raised");
        self.events.lock().unwrap().push(event);
    }

    pub fn events(&self) -> Vec<AlertEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// Requirement 6: "`db_busy_total` rises by ≥ 10 within 5 min" -- a
/// trailing window of (timestamp, `db_busy_total`) samples, one appended
/// per minute tick.
const WINDOW_SECS: i64 = 300;
const RISE_THRESHOLD: u64 = 10;

/// One alert per contiguous rise-episode, not one per tick while still
/// elevated: `active` latches once the threshold trips and only clears
/// once a tick's rise drops back under it.
#[derive(Clone, Default)]
pub struct ContentionTracker {
    samples: Arc<Mutex<VecDeque<(i64, u64)>>>,
    active: Arc<AtomicBool>,
}

impl ContentionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one minute-tick sample and returns `Some(rise)` the first
    /// tick this window's rise crosses [`RISE_THRESHOLD`].
    fn tick(&self, now_unix: i64, busy_total_now: u64) -> Option<u64> {
        let mut samples = self.samples.lock().unwrap();
        samples.push_back((now_unix, busy_total_now));
        while let Some(&(ts, _)) = samples.front() {
            if now_unix - ts > WINDOW_SECS {
                samples.pop_front();
            } else {
                break;
            }
        }
        let oldest = samples.front().map(|&(_, v)| v).unwrap_or(busy_total_now);
        let rise = busy_total_now.saturating_sub(oldest);
        if rise >= RISE_THRESHOLD {
            if self.active.swap(true, Ordering::Relaxed) {
                None
            } else {
                Some(rise)
            }
        } else {
            self.active.store(false, Ordering::Relaxed);
            None
        }
    }
}

/// One minute-tick cycle: sum `db_busy_total` across every [`crate::db::DbRole`],
/// feed it to `state.contention_tracker`, and raise `db.contention` through
/// `state.alerts` on a fresh rise. Exposed as a single deterministic step
/// (same "swap a short interval in for the real cadence" convention as
/// `retention::spawn_prune_scheduler_for_test`) so a test doesn't wait out
/// the real 60s loop. Named distinctly from this module's own async
/// `tick_once` (the `errors.rate` source above) -- unrelated ticks that
/// happen to share a module, not two names for the same cycle.
pub fn contention_tick_once(state: &crate::state::AppState) {
    let now = crate::state::now_unix();
    let busy_total: u64 = crate::db::ALL_ROLES
        .iter()
        .map(|&role| state.db.counters(role).busy_total)
        .sum();
    if let Some(rise) = state.contention_tracker.tick(now, busy_total) {
        state.alerts.raise(
            "db.contention",
            serde_json::json!({
                "rise": rise,
                "window_secs": WINDOW_SECS,
                "busy_total": busy_total,
            }),
        );
    }
}

pub fn spawn_contention_tick(state: crate::state::AppState) -> tokio::task::JoinHandle<()> {
    spawn_contention_tick_loop(state, None)
}

/// Test-only: same loop `spawn_contention_tick` runs at real `serve`
/// startup, but firing every `interval` instead of waiting out the real 60s
/// cadence.
pub fn spawn_contention_tick_for_test(
    state: crate::state::AppState,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_contention_tick_loop(state, Some(interval))
}

fn spawn_contention_tick_loop(
    state: crate::state::AppState,
    interval_override: Option<Duration>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval_override.unwrap_or(Duration::from_secs(60))).await;
            contention_tick_once(&state);
        }
    })
}
