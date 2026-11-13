//! PRD-mcphost-sandbox-pool-self-restart-v2: the executor behind
//! `HealAction::SandboxPoolRestart`. One subscriber task consumes
//! `SignalEvent::Breach` for `Signal::SandboxPool`, drains the python kind's
//! warm pool through `Kind::pool_restart`, and writes one ledger row through
//! `heal::execute_on`; `/status.json.sandbox_pool` reads the pool's state.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::errors::AppError;
use crate::heal::{self, HealAction, HealOutcome, HealRow};
use crate::kinds::{PoolCounts, PoolRestartRequest};
use crate::signals::{Signal, SignalEvent};
use crate::state::{AppState, now_unix};

/// The component the limit-breach incident names (the one `probe_exec` feeds).
const INCIDENT_COMPONENT: &str = "exec";

/// `heal.toml`'s default `wedge_factor` for `[signals.sandbox_pool]`.
const DEFAULT_WEDGE_FACTOR: f64 = 3.0;

/// R2: starts the subscriber beside the status tick. The receiver is taken
/// before the task is spawned, so a breach recorded right after this returns
/// is not missed.
pub fn spawn_subscriber(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.signals.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(SignalEvent::Breach { signal: Signal::SandboxPool, value, .. }) => {
                    if let Err(e) = on_breach(&state, value).await {
                        tracing::warn!(error = %e, "sandbox pool restart failed to run");
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    })
}

fn counts_json(c: PoolCounts) -> serde_json::Map<String, Value> {
    let Value::Object(map) = json!({"wedged": c.wedged, "pool_size": c.pool_size, "in_flight_count": c.in_flight_count})
    else {
        unreachable!("json! object literal")
    };
    map
}

/// R3/R4: runs one restart for a breach at `ratio`. `Ok(None)` when the host
/// has no python kind.
pub async fn on_breach(state: &AppState, ratio: f64) -> Result<Option<HealRow>, AppError> {
    let Some(kind) = state.kinds.get("python") else {
        return Ok(None);
    };
    let threshold = state.signals.config().threshold(Signal::SandboxPool);
    let wedge_factor = threshold.wedge_factor.unwrap_or(DEFAULT_WEDGE_FACTOR);
    let drain_budget = Duration::from_secs(threshold.drain_budget_s.unwrap_or(crate::kinds::python::DEFAULT_DRAIN_BUDGET_S));
    let Some(pool) = kind.pool_signals(wedge_factor) else {
        return Ok(None);
    };
    // `flights` is outside `heal::PUBLIC_KEY_STEMS`: `/incidents.json` never
    // carries a tenant or tool name.
    let trigger = json!({
        "signal": Signal::SandboxPool.name(),
        "ratio": ratio,
        "wedged": pool.wedged,
        "flights": pool.wedged_keys,
    });
    let plan_before = Value::Object(counts_json(PoolCounts {
        wedged: pool.wedged,
        pool_size: pool.pool_size,
        in_flight_count: pool.in_flight_count,
    }));
    let request = PoolRestartRequest { wedge_factor, drain_budget };
    let row = heal::execute_on(
        &state.db,
        HealAction::SandboxPoolRestart,
        trigger,
        now_unix(),
        heal::dry_run_enabled(),
        plan_before,
        || async {
            let evidence = match kind.pool_restart(request).await {
                Some(Ok(evidence)) => evidence,
                Some(Err(reason)) => return Err(AppError::Internal(reason)),
                None => return Err(AppError::Internal("the python kind has no sandbox pool".to_string())),
            };
            let mut after = counts_json(evidence.after);
            after.insert("stderr_tails".to_string(), json!(evidence.stderr_tails));
            Ok(json!({"before": counts_json(evidence.before), "after": after}))
        },
    )
    .await?;
    // The calls the restart cancelled are waiting for this id.
    kind.pool_restart_published(row.id);
    if row.outcome == HealOutcome::RateLimited {
        let title = "sandbox pool wedged and the hourly restart limit is spent";
        crate::statusfeed::open_incident_unless_named(state, INCIDENT_COMPONENT, title, false).await?;
    }
    Ok(Some(row))
}

/// R5: `/status.json.sandbox_pool`.
pub async fn status_block(state: &AppState) -> Result<Value, AppError> {
    let wedge_factor = state.signals.config().threshold(Signal::SandboxPool).wedge_factor.unwrap_or(DEFAULT_WEDGE_FACTOR);
    let pool = state.kinds.get("python").and_then(|k| k.pool_signals(wedge_factor)).unwrap_or_default();
    let last_restart = state
        .db
        .latest_heal_row(HealAction::SandboxPoolRestart)
        .await?
        .map(|r| json!({"at": r.at, "outcome": r.outcome.as_str(), "heal_action_id": r.id}));
    Ok(json!({
        "size": pool.pool_size,
        "wedged": pool.wedged,
        "state": if pool.draining { "draining" } else { "ready" },
        "last_restart": last_restart,
    }))
}
