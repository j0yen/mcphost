//! PRD-mcphost-performance-guarantees-measured: per-plan p95 targets
//! ([`crate::plans::Plan`]'s `perf_*_p95_ms` fields) and the host's own
//! hourly measurement of them.
//!
//! [`PATHS`] is the one list of measured paths; every `perf_<name>_p95_ms`
//! plan field, every `perf_samples.path` value and every `/status.json`
//! `perf.<plan>.<name>` row is keyed by a name in it, and
//! `tests/pgm_drift.rs` asserts the plan fields cannot drift from it.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::db::PerfSample;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// Probe calls per (plan, path) per cycle.
pub const SAMPLES_PER_CYCLE: usize = 20;
/// The hourly cycle's real period.
pub const CYCLE_INTERVAL: Duration = Duration::from_secs(3600);
/// `perf_samples` rows older than this are pruned each cycle.
pub const RETENTION_DAYS: i64 = 30;
/// R4: `state` is `over` when this many consecutive newest samples all
/// exceed the target.
pub const OVER_STREAK: usize = 3;
/// R7: a cycle is skipped when the python kind has fewer free host-wide
/// permits than this.
pub const MIN_FREE_PERMITS: usize = 2;
/// The reserved internal tenants (one per plan) are `perf-<plan>`.
pub const TENANT_PREFIX: &str = "perf-";

/// `perf_samples.plan` values, in `/status.json` order.
pub const PLANS: [&str; 2] = ["free", "pro"];

pub type ProbeFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
/// One probe call under the internal tenant `(tenant_id, namespace)`;
/// `true` when the call succeeded.
pub type ProbeFn = for<'a> fn(&'a AppState, i64, &'a str) -> ProbeFuture<'a>;

/// R2: the measured paths. Each name is the `<name>` of the plan field
/// `perf_<name>_p95_ms`.
pub const PATHS: [(&str, ProbeFn); 3] = [
    ("echo", probe_echo),
    ("python_cold", probe_python_cold),
    ("http", probe_http),
];

/// The plan-field name for a path (`echo` -> `perf_echo_p95_ms`).
pub fn target_field(path: &str) -> String {
    format!("perf_{path}_p95_ms")
}

/// The path name a `perf_*_p95_ms` field measures, if it is one.
pub fn path_of_field(field: &str) -> Option<&str> {
    field.strip_prefix("perf_")?.strip_suffix("_p95_ms")
}

/// `(p50, p95)` of `samples_ms` -- p95 is the 19th of 20 sorted values
/// (nearest rank, `ceil(0.95 n)`), stored as integers. `None` when empty.
pub fn percentiles(samples_ms: &[i64]) -> Option<(i64, i64)> {
    if samples_ms.is_empty() {
        return None;
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_unstable();
    let rank = |q: f64| {
        let r = ((sorted.len() as f64) * q).ceil() as usize;
        sorted[r.saturating_sub(1).min(sorted.len() - 1)]
    };
    Some((rank(0.50), rank(0.95)))
}

fn probe_echo<'a>(state: &'a AppState, tenant_id: i64, namespace: &'a str) -> ProbeFuture<'a> {
    Box::pin(async move {
        let Some(kind) = state.kinds.get("echo") else {
            return false;
        };
        let spec = serde_json::json!({"schema": {"type": "object"}});
        let ctx = crate::kinds::CallCtx::for_test(tenant_id, namespace);
        kind.call(&spec, serde_json::json!({"ping": 1}), &ctx).await.is_ok()
    })
}

/// A fixed 3-line tool. `tool_name` stays `None`, so the python kind has no
/// warm-pool key and every call starts cold.
fn probe_python_cold<'a>(state: &'a AppState, tenant_id: i64, namespace: &'a str) -> ProbeFuture<'a> {
    Box::pin(async move {
        let Some(kind) = state.kinds.get("python") else {
            return false;
        };
        let spec = serde_json::json!({
            "source": "def main(args):\n    x = 1 + 1\n    return {\"ok\": x == 2}\n"
        });
        let ctx = crate::kinds::CallCtx::for_test(tenant_id, namespace);
        kind.call(&spec, serde_json::json!({}), &ctx).await.is_ok()
    })
}

static SELF_URL: OnceLock<String> = OnceLock::new();

/// Set once at `serve` start to the listener's loopback base URL.
pub fn set_self_url(base: impl Into<String>) {
    let _ = SELF_URL.set(base.into());
}

fn self_url() -> String {
    SELF_URL.get().cloned().unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
}

/// A round trip to this host's own `/healthz` over loopback. The `http`
/// kind refuses loopback by design, so the probe uses a plain client.
fn probe_http<'a>(_state: &'a AppState, _tenant_id: i64, _namespace: &'a str) -> ProbeFuture<'a> {
    Box::pin(async move {
        let url = format!("{}/healthz", self_url());
        let client = reqwest::Client::new();
        match client.get(&url).timeout(Duration::from_secs(10)).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    })
}

/// Times `n` calls of `probe`, in whole milliseconds.
pub async fn time_probe(state: &AppState, probe: ProbeFn, tenant_id: i64, namespace: &str, n: usize) -> Vec<i64> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let start = Instant::now();
        let _ok = probe(state, tenant_id, namespace).await;
        out.push(start.elapsed().as_millis() as i64);
    }
    out
}

/// R4: `ok` / `over` / `no_data` from a path's newest samples (newest
/// first): `no_data` with none, `over` when the newest [`OVER_STREAK`] all
/// exceed `target_ms`.
pub fn state_of(newest_first_p95: &[i64], target_ms: i64) -> &'static str {
    if newest_first_p95.is_empty() {
        "no_data"
    } else if newest_first_p95.len() >= OVER_STREAK
        && newest_first_p95[..OVER_STREAK].iter().all(|p| *p > target_ms)
    {
        "over"
    } else {
        "ok"
    }
}

/// R4: `/status.json`'s `perf` object, `{<plan>: {<path>: {target_p95_ms,
/// measured_p95_ms, measured_at, state}}}`.
pub async fn status_block(state: &AppState) -> Result<serde_json::Value, AppError> {
    let mut out = serde_json::Map::new();
    for plan in PLANS {
        let ceilings = state.plans.get(plan).map(|p| p.ceilings_json()).unwrap_or_default();
        let mut paths = serde_json::Map::new();
        for (path, _) in PATHS {
            let target = ceilings.get(target_field(path)).and_then(|v| v.as_i64());
            let rows = state.db.recent_perf_samples(plan.to_string(), path.to_string(), OVER_STREAK as i64).await?;
            let p95s: Vec<i64> = rows.iter().map(|r| r.p95_ms).collect();
            let row_state = match target {
                Some(t) => state_of(&p95s, t),
                None => "no_data",
            };
            paths.insert(
                path.to_string(),
                serde_json::json!({
                    "target_p95_ms": target,
                    "measured_p95_ms": rows.first().map(|r| r.p95_ms),
                    "measured_at": rows.first().map(|r| r.ts),
                    "state": row_state,
                }),
            );
        }
        out.insert(plan.to_string(), serde_json::Value::Object(paths));
    }
    Ok(serde_json::Value::Object(out))
}

/// R6: `{<path>: {p95_ms}}` -- the plan's latest measured p95 per path,
/// `null` until a sample exists. `host.whoami`'s `perf_measured`.
pub async fn measured_json(state: &AppState, plan: &str) -> Result<serde_json::Value, AppError> {
    let mut out = serde_json::Map::new();
    for (path, _) in PATHS {
        let rows = state.db.recent_perf_samples(plan.to_string(), path.to_string(), 1).await?;
        out.insert(path.to_string(), serde_json::json!({"p95_ms": rows.first().map(|r| r.p95_ms)}));
    }
    Ok(serde_json::Value::Object(out))
}

/// R6: just the `perf_*_p95_ms` targets of `plan`, the subset of
/// `limits.plan` a `host.whoami` caller needs beside `perf_measured`.
pub fn targets_json(state: &AppState, plan: &str) -> serde_json::Value {
    let ceilings = state.plans.get(plan).map(|p| p.ceilings_json());
    let mut out = serde_json::Map::new();
    for (path, _) in PATHS {
        let field = target_field(path);
        let value = ceilings.as_ref().and_then(|c| c.get(&field)).cloned().unwrap_or(serde_json::Value::Null);
        out.insert(field, value);
    }
    serde_json::Value::Object(out)
}

/// The probe list a cycle runs: `(path name, probe)`.
pub type Probes = Vec<(&'static str, ProbeFn)>;

/// The production probe list.
pub fn default_probes() -> Probes {
    PATHS.to_vec()
}

/// R3: the reserved internal tenant for `plan` (`perf-<plan>`), created on
/// first use. It carries `origin = 'internal'`, a `synthetic` stamp and a
/// `key_hash` no key can hash to, so nobody can authenticate as it.
pub async fn ensure_tenant(state: &AppState, plan: &str) -> Result<(i64, String), AppError> {
    let namespace = format!("{TENANT_PREFIX}{plan}");
    if let Some(t) = state.db.find_tenant_by_namespace(namespace.clone()).await? {
        return Ok((t.id, namespace));
    }
    let tenant = state
        .db
        .create_tenant_attributed(
            namespace.clone(),
            namespace.clone(),
            format!("internal:{namespace}"),
            Some("perf".to_string()),
            None,
            None,
            None,
            "internal".to_string(),
            Some(namespace.clone()),
        )
        .await?;
    if plan != "free" {
        state
            .db
            .upgrade_tenant_plan(tenant.id, plan.to_string(), crate::state::rfc3339_from_unix(now_unix()), None)
            .await?;
    }
    Ok((tenant.id, namespace))
}

/// R7: free host-wide python permits, `None` when no python kind is
/// registered (nothing to contend with).
fn host_free_permits(state: &AppState) -> Option<usize> {
    let snap = state.kinds.get("python")?.admission_snapshot(0, 1)?;
    Some(snap.host_limit.saturating_sub(snap.host_in_flight))
}

/// R3: one cycle -- every path [`SAMPLES_PER_CYCLE`] times under each
/// plan's internal tenant, one `perf_samples` row per (plan, path); then
/// the retention prune. R7: `Ok(false)` -- no probe made, one WARN naming
/// the skip -- when the host-wide semaphore has fewer than
/// [`MIN_FREE_PERMITS`] free permits.
pub async fn run_cycle(state: &AppState, probes: &[(&'static str, ProbeFn)]) -> Result<bool, AppError> {
    if let Some(free) = host_free_permits(state)
        && free < MIN_FREE_PERMITS
    {
        tracing::warn!(free_permits = free, min = MIN_FREE_PERMITS, "perf probe cycle skipped: host-wide semaphore has too few free permits");
        return Ok(false);
    }
    let ts = now_unix();
    for plan in PLANS {
        let (tenant_id, namespace) = ensure_tenant(state, plan).await?;
        for (path, probe) in probes {
            let samples = time_probe(state, *probe, tenant_id, &namespace, SAMPLES_PER_CYCLE).await;
            let Some((p50_ms, p95_ms)) = percentiles(&samples) else {
                continue;
            };
            state
                .db
                .insert_perf_sample(PerfSample {
                    ts,
                    plan: plan.to_string(),
                    path: path.to_string(),
                    p50_ms,
                    p95_ms,
                    n: samples.len() as i64,
                })
                .await?;
        }
    }
    state.db.prune_perf_samples_older_than(ts - RETENTION_DAYS * 86_400).await?;
    Ok(true)
}

/// R3: the hourly task, started once alongside the other background tasks
/// in `main.rs`.
pub fn spawn_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    spawn_loop(state, CYCLE_INTERVAL, default_probes())
}

/// Test-only: the same loop at a short `interval` over `probes`.
pub fn spawn_tick_for_test(state: AppState, interval: Duration, probes: Probes) -> tokio::task::JoinHandle<()> {
    spawn_loop(state, interval, probes)
}

fn spawn_loop(state: AppState, interval: Duration, probes: Probes) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = run_cycle(&state, &probes).await {
                tracing::warn!(error = %e, "perf probe cycle failed");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p95_of_twenty_is_the_nineteenth_sorted_value() {
        let v: Vec<i64> = (1..=20).collect();
        assert_eq!(percentiles(&v), Some((10, 19)));
        assert_eq!(percentiles(&[]), None);
    }

    #[test]
    fn field_names_round_trip() {
        assert_eq!(path_of_field(&target_field("python_cold")), Some("python_cold"));
        assert_eq!(path_of_field("notes_max"), None);
    }
}
