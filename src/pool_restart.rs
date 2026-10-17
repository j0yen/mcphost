//! PRD-mcphost-sandbox-pool-self-restart: the `SandboxPoolRestart` executor.
//! A `sandbox_pool` breach drains the python kind's warm pool, kills the
//! children that stopped answering, and rebuilds it through
//! [`crate::heal::execute_on`], which writes the ledger row.

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::heal::{self, HealAction, HealOutcome, HealRow};
use crate::kinds::{PoolCounts, PoolRestartPlan};
use crate::signals::{Signal, SignalEvent};
use crate::state::{AppState, now_unix};

/// `wedge_factor` when `heal.toml` names none.
const DEFAULT_WEDGE_FACTOR: f64 = 3.0;

/// The auto-incident title a rate-limited restart opens against `exec`.
const RATE_LIMITED_TITLE: &str = "sandbox pool restarts exceeded their hourly limit";

fn counts_json(c: &PoolCounts) -> Value {
    json!({"wedged": c.wedged, "pool": c.pool, "in_flight": c.in_flight})
}

/// The restart plan `heal.toml`'s `[signals.sandbox_pool]` asks for.
pub fn plan(state: &AppState) -> PoolRestartPlan {
    let t = state.signals.config().threshold(Signal::SandboxPool);
    PoolRestartPlan {
        wedge_factor: t.wedge_factor.unwrap_or(DEFAULT_WEDGE_FACTOR),
        drain_budget: std::time::Duration::from_secs(t.drain_budget_s.unwrap_or(30)),
    }
}

/// Runs one restart for `breach` (its `since`/`value` go in the row's
/// trigger). `Ok(None)` when the host has no python kind. A `rate_limited`
/// row opens the `exec` auto-incident (`status.component_down`).
pub async fn run(state: &AppState, breach: &SignalEvent) -> Result<Option<HealRow>, AppError> {
    let Some(kind) = state.kinds.get("python") else {
        return Ok(None);
    };
    let plan = plan(state);
    let Some(view) = kind.pool_view(plan.wedge_factor) else {
        return Ok(None);
    };
    let mut trigger = json!({
        "signal": Signal::SandboxPool.name(),
        "wedged_keys": view.counts.wedged,
        "pool": view.counts.pool,
    });
    if let SignalEvent::Breach { value, threshold, since, .. } = breach {
        trigger["ratio"] = json!(value);
        trigger["threshold"] = json!(threshold);
        trigger["since"] = json!(since);
    }
    let plan_before = counts_json(&view.counts);
    let row = heal::execute_on(
        &state.db,
        HealAction::SandboxPoolRestart,
        trigger,
        now_unix(),
        heal::dry_run_enabled(),
        plan_before,
        || async {
            let report = kind
                .pool_restart(plan)
                .await
                .ok_or_else(|| AppError::Internal("python kind has no sandbox pool".to_string()))?;
            let mut after = counts_json(&report.after);
            after["stderr_tails"] = json!(report.stderr_tails);
            Ok(json!({"before": counts_json(&report.before), "after": after}))
        },
    )
    .await?;
    kind.pool_restart_done(row.id);
    if row.outcome == HealOutcome::RateLimited {
        crate::statusfeed::open_component_incident_once(state, "exec", RATE_LIMITED_TITLE).await?;
    }
    Ok(Some(row))
}

/// `/status.json.sandbox_pool`: `{size, wedged, state, last_restart}`.
pub async fn status_block(state: &AppState) -> Result<Value, AppError> {
    let factor = plan(state).wedge_factor;
    let view = state.kinds.get("python").and_then(|k| k.pool_view(factor));
    let last = state.db.latest_heal_row(HealAction::SandboxPoolRestart).await?;
    Ok(json!({
        "size": view.map_or(0, |v| v.counts.pool),
        "wedged": view.map_or(0, |v| v.counts.wedged),
        "state": view.map_or("ready", |v| v.state),
        "last_restart": last.map(|r| json!({"at": r.at, "outcome": r.outcome.as_str(), "heal_action_id": r.id})),
    }))
}
