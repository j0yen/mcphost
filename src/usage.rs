//! PRD-mcphost-usage-in-every-response: what a call spent and what is left,
//! read from the one counter the `calls_per_day` refusal reads.
//!
//! [`plan_usage_snapshot`] is the only function that turns `count_calls_since`
//! into a `calls_per_day` block. The refusal (`preflight::check_calls_per_day`),
//! `billing.status`, `host.usage`'s `plan`, and every successful result's
//! `_meta.usage` all call it, so the four cannot show different numbers.
//!
//! The hot path pays no second `COUNT`: the `calls_per_day` preflight row
//! already read `used` for this call, parks it in a task-local slot
//! ([`observe_daily`]), and the result builder snapshots `used + 1` (the row
//! this call just wrote) from that hint.

use std::future::Future;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

/// `_meta.usage`'s field names, in render order. `gen-llms-full.sh` and the
/// docs read this list rather than retyping the names.
pub const USAGE_FIELDS: [&str; 5] = ["used", "limit", "remaining", "resets_at", "window_resets_in_s"];

/// One tenant's `calls_per_day` position at an instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanUsage {
    pub used: i64,
    /// The plan row's raw `calls_per_day`; `<= 0` means unlimited.
    pub limit: i64,
    /// Start of the UTC day the count covers.
    pub midnight: i64,
    pub now: i64,
    /// `(used, limit)` of an owner-set per-caller cap, when one applies.
    pub caller_limit: Option<(i64, i64)>,
}

impl PlanUsage {
    pub fn unlimited(&self) -> bool {
        self.limit <= 0
    }

    pub fn remaining(&self) -> Option<i64> {
        (!self.unlimited()).then(|| (self.limit - self.used).max(0))
    }

    pub fn resets_at(&self) -> String {
        crate::state::rfc3339_from_unix(self.midnight + 86_400)
    }

    /// The `calls_per_day` block: `limit`/`remaining` are null on an
    /// unlimited plan row rather than a negative number.
    pub fn calls_per_day_json(&self) -> Value {
        json!({
            "used": self.used,
            "limit": (!self.unlimited()).then_some(self.limit),
            "remaining": self.remaining(),
            "resets_at": self.resets_at(),
            "window_resets_in_s": (self.midnight + 86_400 - self.now).max(0),
        })
    }

    /// The whole `PlanUsage` as JSON: `{calls_per_day, caller_limit?}`.
    pub fn to_json(&self) -> Value {
        let mut out = json!({"calls_per_day": self.calls_per_day_json()});
        if let Some((used, limit)) = self.caller_limit {
            out["caller_limit"] = json!({"used": used, "limit": limit});
        }
        out
    }
}

/// `tenant`'s `calls_per_day` snapshot. `used_hint` is a count the caller
/// already holds (the call path's own pre-call read plus one); with it no
/// query runs, without it `count_calls_since` does -- the same read the
/// refusal makes.
pub async fn plan_usage_snapshot(
    state: &AppState,
    tenant: &Tenant,
    used_hint: Option<i64>,
) -> Result<PlanUsage, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let now = crate::state::now_unix();
    let midnight = crate::state::utc_midnight_unix(now);
    let used = match used_hint {
        Some(used) => used,
        None => state.db.count_calls_since(tenant.id, midnight, true).await?,
    };
    Ok(PlanUsage { used, limit: plan.calls_per_day, midnight, now, caller_limit: None })
}

/// What the admission checks read for the call in flight.
#[derive(Debug, Default, Clone, Copy)]
pub struct Observed {
    /// The billed tenant's `calls_per_day` count BEFORE this call.
    pub daily_used: Option<i64>,
    /// `(used before this call, limit)` of the caller cap, when one exists.
    pub caller: Option<(i64, i64)>,
}

tokio::task_local! {
    static SLOT: Mutex<Observed>;
}

/// Runs `fut` with a fresh [`Observed`] slot.
pub async fn scope<F: Future>(fut: F) -> F::Output {
    SLOT.scope(Mutex::new(Observed::default()), fut).await
}

/// `calls_per_day` preflight row: record the count it just read.
pub fn observe_daily(used: i64) {
    let _ = SLOT.try_with(|s| s.lock().unwrap_or_else(|e| e.into_inner()).daily_used = Some(used));
}

/// `caller_limit` preflight row: record the count and cap it just read.
pub fn observe_caller(used: i64, limit: i64) {
    let _ = SLOT.try_with(|s| s.lock().unwrap_or_else(|e| e.into_inner()).caller = Some((used, limit)));
}

/// Takes what the call's own checks observed (if any) out of the slot.
pub fn take_observed() -> Option<Observed> {
    SLOT.try_with(|s| std::mem::take(&mut *s.lock().unwrap_or_else(|e| e.into_inner()))).ok()
}

/// `MCPHOST_RESULT_USAGE=off` removes `_meta.usage` for operators who want
/// minimal results.
pub fn result_usage_enabled() -> bool {
    !std::env::var("MCPHOST_RESULT_USAGE").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
}

/// `_meta.usage` for a call that just succeeded for `billed`, or `None` when
/// its checks observed no `calls_per_day` read (a call that writes no
/// `calls` row), or the operator turned the field off.
pub async fn result_usage(state: &AppState, billed: &Tenant) -> Option<Value> {
    if !result_usage_enabled() {
        return None;
    }
    let observed = take_observed()?;
    let used_before = observed.daily_used?;
    let mut usage = plan_usage_snapshot(state, billed, Some(used_before + 1)).await.ok()?;
    usage.caller_limit = observed.caller.map(|(used, limit)| (used + 1, limit));
    Some(usage.to_json())
}
