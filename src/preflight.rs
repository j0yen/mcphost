//! PRD-mcphost-call-cost-estimate: the one list of admission checks a call
//! passes before it dispatches. `McpHostHandler::host_tool_call` /
//! `call_published_tool` (the real call) and the `dry_run: true` /
//! `host.estimate` path both run [`PREFLIGHT_CHECKS`]; neither path may call
//! a check outside it (`tests/cest_ac02_*` scans `src/handler.rs` for that),
//! so a dry run's verdict cannot drift from the real call's admission.
//!
//! The checks run in two stages because the real path needs the budget
//! ceiling before it looks the tool up (an over-ceiling `budget` is refused
//! whether or not the tool exists) and everything else after the tool is
//! resolved: [`Stage::PreResolve`] then [`resolve`] then [`Stage::Resolved`].

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::db::{Tenant, ToolRow};
use crate::errors::AppError;
use crate::kinds::{Kind, describe_args_error};
use crate::state::AppState;

/// Whether the list runs for a call that will dispatch or for an estimate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Real,
    DryRun,
}

/// Where in the call path a row runs (see the module doc).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    PreResolve,
    Resolved,
}

/// Which refusals the real path writes a `calls` ledger row for (the same
/// `record_refused_call` behavior each check had before it moved here).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefusalLog {
    Never,
    Always,
    StructuredOnly,
}

impl RefusalLog {
    pub fn records(self, err: &AppError) -> bool {
        match self {
            RefusalLog::Never => false,
            RefusalLog::Always => true,
            RefusalLog::StructuredOnly => matches!(err, AppError::Structured { .. }),
        }
    }
}

/// The tool a call resolved to: its row plus the kind/spec that will run
/// (a pinned `version` swaps in that version's own).
pub struct Target {
    pub row: ToolRow,
    pub kind_name: String,
    pub spec: Value,
    pub kind: Arc<dyn Kind>,
}

/// A refusal plus whether the real path records it in the `calls` ledger.
pub struct Refusal {
    pub err: AppError,
    pub log: RefusalLog,
}

impl Refusal {
    pub fn records(&self) -> bool {
        self.log.records(&self.err)
    }
}

pub struct Ctx<'a> {
    pub state: &'a AppState,
    pub mode: Mode,
    /// The tool's owner (whose sandbox runs it).
    pub owner: &'a Tenant,
    /// `Some` for a cross-tenant call: the caller's quota is the one checked.
    pub caller: Option<&'a Tenant>,
    pub tool: &'a str,
    pub args: &'a Value,
    pub budget: Option<&'a Value>,
    /// `None` until [`resolve`] has run.
    pub target: Option<&'a Target>,
}

impl Ctx<'_> {
    /// The tenant whose plan and quota this call is billed to.
    pub fn billed(&self) -> &Tenant {
        self.caller.unwrap_or(self.owner)
    }
}

pub type CheckFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

pub struct PreflightCheck {
    pub id: &'static str,
    pub stage: Stage,
    pub log: RefusalLog,
    pub run: for<'a> fn(&'a Ctx<'a>) -> CheckFuture<'a>,
}

/// Every admission check, in the order a call meets them.
pub static PREFLIGHT_CHECKS: &[PreflightCheck] = &[
    PreflightCheck { id: "budget_ceiling", stage: Stage::PreResolve, log: RefusalLog::Never, run: budget_ceiling },
    PreflightCheck { id: "args_schema", stage: Stage::Resolved, log: RefusalLog::Always, run: args_schema },
    PreflightCheck { id: "calls_per_day", stage: Stage::Resolved, log: RefusalLog::StructuredOnly, run: calls_per_day },
    PreflightCheck { id: "caller_limit", stage: Stage::Resolved, log: RefusalLog::Always, run: caller_limit },
    PreflightCheck { id: "capacity", stage: Stage::Resolved, log: RefusalLog::Never, run: capacity },
];

/// Runs the rows of `stage` in order; the first refusal wins.
pub async fn run_stage(ctx: &Ctx<'_>, stage: Stage) -> Result<(), Refusal> {
    for check in PREFLIGHT_CHECKS.iter().filter(|c| c.stage == stage) {
        (check.run)(ctx).await.map_err(|err| Refusal { err, log: check.log })?;
    }
    Ok(())
}

/// Looks `local_name` up under `owner` and picks the kind/spec that will
/// run. These are the lookups every call needs to reach the checks, not
/// admission rows themselves; a missing tool is `tool_not_found` (which
/// `host.tool_call` then tries as a host verb).
pub async fn resolve(
    state: &AppState,
    owner: &Tenant,
    local_name: &str,
    version: Option<i64>,
    token_scope: Option<&str>,
) -> Result<Target, Refusal> {
    let never = |err| Refusal { err, log: RefusalLog::Never };
    let always = |err| Refusal { err, log: RefusalLog::Always };
    let row: ToolRow = state
        .db
        .get_tool(owner.id, local_name.to_string())
        .await
        .map_err(never)?
        .ok_or_else(|| never(AppError::ToolNotFound(local_name.to_string())))?;
    // Read fresh off `row` on every call (never cached), so a scope change
    // on republish takes effect on the very next call.
    if !crate::oauth::scope_satisfied(token_scope, &row.scopes) {
        let scope = crate::oauth::union_scope_challenge(state, owner.id, token_scope, &row.scopes).await;
        return Err(always(AppError::Structured {
            code: crate::scopes::INSUFFICIENT_SCOPE,
            message: format!("'{local_name}' requires a scope this token does not carry"),
            data: json!({"error_code": crate::scopes::INSUFFICIENT_SCOPE, "tool": local_name, "scope": scope}),
        }));
    }
    let (kind_name, spec): (String, Value) = match version {
        Some(v) => match state.db.get_tool_version(owner.id, local_name.to_string(), v).await.map_err(never)? {
            Some(vrow) => (vrow.kind, vrow.spec),
            None => {
                let (min, max) = state
                    .db
                    .tool_version_range(owner.id, local_name.to_string())
                    .await
                    .map_err(never)?
                    .unwrap_or((row.current_version, row.current_version));
                return Err(always(AppError::VersionNotFound { requested: v, min, max }));
            }
        },
        None => (row.kind.clone(), row.spec.clone()),
    };
    let kind: Arc<dyn Kind> = state.kinds.get(&kind_name).ok_or_else(|| {
        never(AppError::Internal(format!("published tool names unregistered kind '{kind_name}'")))
    })?;
    Ok(Target { row, kind_name, spec, kind })
}

fn budget_ceiling<'a>(ctx: &'a Ctx<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let budget_arg = ctx.budget;
        if budget_arg.is_some_and(|v| !v.is_null()) {
            let tenant = ctx.billed();
            let plan = ctx.state.plans.get(&tenant.plan).ok_or_else(|| {
                AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
            })?;
            crate::budget::resolve_and_validate(ctx.state, plan, budget_arg)?;
        }
        Ok(())
    })
}

fn args_schema<'a>(ctx: &'a Ctx<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let Some(target) = ctx.target else { return Ok(()) };
        // `chain` handles its own args-completeness check inside
        // `Kind::call` (see `Kind::validates_own_args`).
        if target.kind.validates_own_args() {
            return Ok(());
        }
        let descriptor = target.kind.describe(&target.spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(ctx.args)
        {
            // The wire code for a schema-invalid call to a published tool is
            // `args_invalid`, phase `args_coercion` (see `describe_args_error`).
            let args_err = describe_args_error(&e);
            return Err(AppError::Structured {
                code: "args_invalid",
                message: args_err.message(),
                data: args_err.data(),
            });
        }
        Ok(())
    })
}

/// `used`/`limit`/`resets_at` of `tenant`'s `calls_per_day`, read through the
/// one function (`count_calls_since`) the refusal reads, so the dry run's
/// `remaining` can never be a second query with its own window.
pub struct DailyCalls {
    pub used: i64,
    pub limit: i64,
    pub midnight: i64,
}

impl DailyCalls {
    pub fn remaining(&self) -> i64 {
        (self.limit - self.used).max(0)
    }

    pub fn resets_at(&self) -> String {
        crate::state::rfc3339_from_unix(self.midnight + 86_400)
    }
}

pub async fn daily_calls(state: &AppState, tenant: &Tenant) -> Result<DailyCalls, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let midnight = crate::state::utc_midnight_unix(crate::state::now_unix());
    let used = state.db.count_calls_since(tenant.id, midnight, true).await?;
    Ok(DailyCalls { used, limit: plan.calls_per_day, midnight })
}

/// `calls_per_day`: reject with `quota_exceeded` once `tenant` has made
/// `plan.calls_per_day` successful calls since the most recent UTC midnight.
/// Shared by every path that reaches `call_published_tool` (a direct
/// namespaced call and `host.tool_call` both go through it), by
/// `host.spec_test` and `host.tool_spec_shared` (via [`check_calls_per_day`]);
/// `host.tool_test`/`host.tool_run` deliberately do not run it -- neither
/// writes a `calls` row.
pub async fn check_calls_per_day(state: &AppState, tenant: &Tenant, mode: Mode) -> Result<(), AppError> {
    let daily = daily_calls(state, tenant).await?;
    if daily.used >= daily.limit {
        // A `quota.trip` alert once the same plan knob trips
        // `MCPHOST_ALERT_QUOTA_TRIP_THRESHOLD` (default 20) times in 5
        // minutes for this tenant. A dry run is a question, not a trip.
        if mode == Mode::Real {
            let knob = "calls_per_day";
            let trips = state.alert_quota_trips.record(tenant.id, knob);
            if trips >= state.alert_config.quota_trip_threshold {
                let key = format!("quota.trip:{}:{knob}", tenant.namespace);
                if let Err(e) = crate::alerts::raise(
                    state,
                    crate::alerts::RaiseInput {
                        key,
                        severity: crate::alerts::Severity::Warn,
                        title: format!("{} tripped {knob} repeatedly", tenant.namespace),
                        body: json!({
                            "tenant": tenant.namespace,
                            "knob": knob,
                            "trips_in_window": trips,
                        }),
                    },
                )
                .await
                {
                    tracing::warn!(error = %e, tenant = %tenant.namespace, knob, "failed to raise quota.trip alert");
                }
            }
        }
        return Err(crate::billing::quota_exceeded(
            state,
            &tenant.plan,
            "calls_per_day",
            daily.limit,
            daily.used,
            Some(daily.resets_at()),
        ));
    }
    Ok(())
}

fn calls_per_day<'a>(ctx: &'a Ctx<'a>) -> CheckFuture<'a> {
    // A cross-tenant call counts against the CALLER's quota, not the owner's.
    Box::pin(async move { check_calls_per_day(ctx.state, ctx.billed(), ctx.mode).await })
}

/// An owner-set per-caller cap, checked only for a cross-tenant call (an
/// owner calling its own tool has no `caller_tenant_id` to key a limit on).
fn caller_limit<'a>(ctx: &'a Ctx<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let Some(caller) = ctx.caller else { return Ok(()) };
        let Some(limit) = ctx
            .state
            .db
            .get_caller_limit(ctx.owner.id, ctx.tool.to_string(), caller.id)
            .await?
        else {
            return Ok(());
        };
        let midnight = crate::state::utc_midnight_unix(crate::state::now_unix());
        let used = ctx
            .state
            .db
            .count_caller_calls_since(ctx.owner.id, ctx.tool.to_string(), caller.id, midnight)
            .await?;
        if used >= limit {
            let reset_at = crate::state::rfc3339_from_unix(midnight + 86_400);
            return Err(crate::sharing::quota_caller(&caller.namespace, ctx.tool, limit, used, reset_at));
        }
        Ok(())
    })
}

/// Capacity is the one row whose real check is an acquire that must hold the
/// slot until the call ends, so it lives in `Kind::call` and the real path
/// passes through here. A dry run reads it ([`Kind::admission_capacity`]) --
/// it never takes a slot.
fn capacity<'a>(ctx: &'a Ctx<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        if ctx.mode == Mode::Real {
            return Ok(());
        }
        let Some(target) = ctx.target else { return Ok(()) };
        let cap = ctx
            .state
            .plans
            .get(&ctx.owner.plan)
            .map(|plan| plan.concurrent_calls_per_tenant.max(0) as usize)
            .unwrap_or(usize::MAX);
        match target.kind.admission_capacity(ctx.owner.id, cap) {
            Some(kind_err) => Err(AppError::from(kind_err)),
            None => Ok(()),
        }
    })
}

/// Per-tenant record of recent dry runs, for `host.usage`'s `estimates`.
/// In memory by design: the PRD adds no schema, and a dry run is a free
/// question, not a ledger event.
#[derive(Clone, Default)]
pub struct EstimateCounter {
    windows: Arc<Mutex<HashMap<i64, VecDeque<i64>>>>,
}

/// The longest window `host.usage` can be asked for that is worth keeping.
const RETAIN_SECS: i64 = 7 * 86_400;

impl EstimateCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, tenant_id: i64) {
        let now = crate::state::now_unix();
        let mut guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.entry(tenant_id).or_default();
        window.push_back(now);
        while window.front().is_some_and(|&t| now - t > RETAIN_SECS) {
            window.pop_front();
        }
    }

    /// Dry runs by `tenant_id` in the last `secs` seconds.
    pub fn count(&self, tenant_id: i64, secs: i64) -> i64 {
        let cutoff = crate::state::now_unix() - secs;
        let guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get(&tenant_id)
            .map(|w| w.iter().filter(|&&t| t >= cutoff).count() as i64)
            .unwrap_or(0)
    }
}
