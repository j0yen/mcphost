//! PRD-mcphost-runs-and-jobs: the `runs` ledger's tenant/admin-facing
//! business logic (`host.runs.*`, `admin.runs*`) plus the executor that
//! leases and runs queued jobs. Pure `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out for the RPC-shaped functions,
//! same convention as `control.rs`/`tenant_state.rs` -- `handler.rs` is the
//! only place that touches `rmcp` wire types.
//!
//! The executor (`spawn_executor`) is the one background task besides
//! `kinds::python`'s warm-pool reaper; it shares nothing with that reaper
//! except the general "a tokio task started once at `mcphost serve`
//! startup" shape.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::db::{RunRow, Tenant};
use crate::errors::AppError;
use crate::handler::{BufferedLog, CellResourceSink, CountingStateBackend, TenantStateBridge, build_secret_resolver};
use crate::kinds::{CallCtx, CallLog, ProgressSink, ResourceSink};
use crate::state::AppState;

/// P0 requirement 4: the host-wide ceiling on concurrently-`running` jobs,
/// independent of any one tenant's own `jobs_concurrent` plan cap ("host
/// ceiling from a constant").
pub const JOBS_HOST_CEILING: usize = 20;

/// How often the executor's leasing loop scans for queued work. AC1's
/// "starts within one second when a job slot is free" needs this well
/// under 1s; 250ms gives four chances a second with negligible idle-poll
/// cost (a `SELECT DISTINCT tenant_id` and, per tenant with queued work,
/// one `COUNT(*)` -- cheap even at a few hundred tenants).
const EXECUTOR_TICK: Duration = Duration::from_millis(250);

/// At most one progress write per second per run (P0 requirement 5).
const PROGRESS_WRITE_MIN_INTERVAL: Duration = Duration::from_millis(1000);

/// One run's cancel-pid slot -- `None` when queued or between sandbox
/// round trips, `Some(pid)` while a sandboxed subprocess is actually live.
/// The SAME `Arc` a job's own `CallCtx.cancel_pid` holds, so a pid
/// `kinds::python` sets mid-call is visible here without any extra
/// plumbing.
type CancelPidSlot = Arc<Mutex<Option<i32>>>;

/// PRD-mcphost-runs-and-jobs requirement 4 / AC4: the live run-id ->
/// cancel-pid registry `host.runs.cancel` reaches through. Registered by
/// the executor immediately before dispatch, removed immediately after
/// (whatever the outcome) -- a run id absent from this map is never a bug,
/// only "not currently executing" (queued, or already terminal).
#[derive(Clone, Default)]
pub struct RunsRegistry {
    running: Arc<Mutex<HashMap<String, CancelPidSlot>>>,
}

impl RunsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn register(&self, run_id: &str) -> CancelPidSlot {
        let slot = Arc::new(Mutex::new(None));
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run_id.to_string(), slot.clone());
        slot
    }

    fn unregister(&self, run_id: &str) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(run_id);
    }

    /// `host.runs.cancel`'s own kill switch: `Some(pid)` only when this run
    /// is both currently dispatched AND mid-sandbox-round-trip right now
    /// (a job between two sandbox calls, or one whose kind has no
    /// subprocess at all, reports `None` here -- nothing to `killpg`,
    /// correctly).
    fn live_pid(&self, run_id: &str) -> Option<i32> {
        let map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let slot = map.get(run_id)?;
        *slot.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn arg_i64_opt(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

fn run_to_json(run: &RunRow) -> Value {
    let progress = run
        .progress_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    json!({
        "run_id": run.id,
        "tool": run.tool_name,
        "trigger": run.trigger,
        "trigger_ref": run.trigger_ref,
        "status": run.status,
        "progress": progress,
        "result": Value::Null,
        "purged": run.purged_unix.is_some(),
        "error_class": run.error_class,
        "started_unix": run.started_unix,
        "finished_unix": run.finished_unix,
        "duration_ms": run.duration_ms,
        "deadline_s": run.deadline_s,
        "attempt": run.attempt,
        // PRD-mcphost-schedules P1 requirement 7 / AC10.
        "manual": run.manual,
        // PRD-mcphost-inbound-events P0 requirement 3 / AC7.
        "test": run.test,
    })
}

/// `host.runs.get(run_id)`, inlining the stored result (P0 requirement 7:
/// "`host.runs.get` inlines it") when one is present and not yet purged.
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let run_id = arg_str(args, "run_id")?;
    let run = state
        .db
        .get_run(run_id.clone(), tenant.id)
        .await?
        .ok_or_else(|| AppError::Structured {
            code: "run_not_found",
            message: format!("no run '{run_id}' for this tenant"),
            data: json!({"run_id": run_id}),
        })?;
    let mut value = run_to_json(&run);
    if let Some(result_ref) = &run.result_ref
        && let Some((value_json, _)) = state.db.state_kv_get(tenant.id, result_ref.clone()).await?
    {
        value["result"] = serde_json::from_str(&value_json).unwrap_or(Value::Null);
    }
    Ok(value)
}

/// `host.runs.list(tool?, status?, trigger?, limit?)`.
pub async fn list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let tool = arg_str_opt(args, "tool");
    let status = arg_str_opt(args, "status");
    let trigger = arg_str_opt(args, "trigger");
    let limit = arg_i64_opt(args, "limit").unwrap_or(20).clamp(1, 200);
    let rows = state
        .db
        .list_runs(tenant.id, tool, status, trigger, limit)
        .await?;
    Ok(json!({"runs": rows.iter().map(run_to_json).collect::<Vec<_>>()}))
}

/// `host.runs.cancel(run_id)` (AC4). Marks the run `cancelled` in the
/// ledger (conditional on it still being `queued`/`running` -- a terminal
/// run's cancel is a structured `run_not_cancellable`, not a silent
/// no-op), then, if it was `running` with a live sandbox pid registered,
/// kills that process group directly -- independent of whatever the
/// executor's own `Kind::call` future is doing, so the kill lands even
/// though `Kind::call` has no cooperative-cancellation contract of its own.
pub async fn cancel(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let run_id = arg_str(args, "run_id")?;
    let pid = state.runs.live_pid(&run_id);
    let before = state
        .db
        .cancel_run(run_id.clone(), tenant.id)
        .await?
        .ok_or_else(|| AppError::Structured {
            code: "run_not_cancellable",
            message: "run is already terminal, or does not exist for this tenant".to_string(),
            data: json!({"run_id": run_id}),
        })?;
    let killed = if before.status == "running" {
        if let Some(pid) = pid {
            // `pid` is a process-group leader `kinds::python` itself set
            // up via `setsid` (see `sandbox.rs`'s `pre_exec_setup`); killing
            // it is the same operation `PersistentSandbox::kill` performs.
            // SAFETY: killpg's only precondition is a valid pid; `pid` is
            // SAFETY: one by construction above.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
            true
        } else {
            false
        }
    } else {
        false
    };
    Ok(json!({"run_id": run_id, "cancelled": true, "killed": killed}))
}

/// `host.runs.purge(before_unix)` (AC7): clears stored results for this
/// tenant's `done` runs finished at or before `before_unix`.
pub async fn purge(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let before_unix = args
        .get("before_unix")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'before_unix'".to_string()))?;
    let refs = state.db.purge_runs(tenant.id, before_unix).await?;
    for r in &refs {
        let _ = state.db.state_kv_delete(tenant.id, r.clone()).await;
    }
    Ok(json!({"purged": refs.len()}))
}

/// P1 requirement 9: `host.runs.wait(run_id, timeout_s <= 25)` long-polls
/// (short-sleep loop rather than a wake channel -- the executor already
/// polls at [`EXECUTOR_TICK`] granularity, so a coarser client-facing poll
/// costs nothing extra) until the run finalizes or `timeout_s` elapses,
/// returning the current status either way (AC10).
pub async fn wait(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let run_id = arg_str(args, "run_id")?;
    let timeout_s = arg_i64_opt(args, "timeout_s").unwrap_or(20).clamp(1, 25);
    let deadline = Instant::now() + Duration::from_secs(timeout_s as u64);
    loop {
        let run = state
            .db
            .get_run(run_id.clone(), tenant.id)
            .await?
            .ok_or_else(|| AppError::Structured {
                code: "run_not_found",
                message: format!("no run '{run_id}' for this tenant"),
                data: json!({"run_id": run_id}),
            })?;
        let terminal = matches!(run.status.as_str(), "done" | "error" | "timeout" | "cancelled");
        if terminal || Instant::now() >= deadline {
            let mut value = run_to_json(&run);
            if let Some(result_ref) = &run.result_ref
                && let Some((value_json, _)) =
                    state.db.state_kv_get(tenant.id, result_ref.clone()).await?
            {
                value["result"] = serde_json::from_str(&value_json).unwrap_or(Value::Null);
            }
            return Ok(value);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// `admin.runs(tenant?, status?, limit?)`.
pub async fn admin_runs(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant_id = if let Some(ns) = arg_str_opt(args, "tenant") {
        let tenant = state
            .db
            .find_tenant_by_namespace(ns.clone())
            .await?
            .ok_or(AppError::TenantNotFound(ns))?;
        Some(tenant.id)
    } else {
        None
    };
    let status = arg_str_opt(args, "status");
    let limit = arg_i64_opt(args, "limit").unwrap_or(50).clamp(1, 500);
    let rows = state.db.admin_list_runs(tenant_id, status, limit).await?;
    Ok(json!({"runs": rows.iter().map(run_to_json).collect::<Vec<_>>()}))
}

/// `admin.runs_reap` (P1 requirement 10, AC11), also called once at
/// `mcphost serve` startup (see `main.rs`).
pub async fn admin_runs_reap(state: &AppState) -> Result<Value, AppError> {
    let reaped = state.db.reap_expired_runs().await?;
    Ok(json!({"reaped": reaped}))
}

/// P0 requirement 3: `host.tool_call(..., async=true)` delegates here.
/// Validates exactly the same way a synchronous `call_published_tool`
/// would (schema against the tool's own `args_schema`) so a malformed
/// async call fails immediately, synchronously, rather than silently
/// sitting `queued` forever to fail invisibly once the executor picks it
/// up. AC1: returns within 50ms -- a single-row insert, no sandbox touched.
pub async fn enqueue(
    state: &AppState,
    tenant: &Tenant,
    local_name: &str,
    args: Value,
) -> Result<Value, AppError> {
    let row = state
        .db
        .get_tool(tenant.id, local_name.to_string())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(local_name.to_string()))?;
    let kind = state.kinds.get(&row.kind).ok_or_else(|| {
        AppError::Internal(format!(
            "published tool names unregistered kind '{}'",
            row.kind
        ))
    })?;
    let descriptor = kind.describe(&row.spec);
    if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
        && let Err(e) = validator.validate(&args)
    {
        let data = crate::kinds::describe_args_error(&e);
        return Err(AppError::Structured {
            code: "args_invalid",
            message: e.to_string(),
            data,
        });
    }
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let deadline_s = plan.job_max_s;
    let run_id = crate::state::new_ulid();
    let args_json = serde_json::to_string(&args)
        .map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            local_name.to_string(),
            "job".to_string(),
            None,
            None,
            deadline_s,
            args_json,
            false,
            false,
        )
        .await?;
    Ok(json!({"run_id": run_id, "status": "queued"}))
}

/// Bridges `ctx.progress` (P0 requirement 5) to a throttled (at most
/// once/second) `runs.progress_json` write. Holds the last-write `Instant`
/// behind a `Mutex` since `ProgressSink::report` is a plain, non-async
/// trait method (`kinds::python`'s sidecar bridge calls it from inside an
/// `async fn`, but the call itself must stay sync -- see the trait's own
/// doc comment); the actual DB write is fired onto the executor's runtime
/// via `tokio::spawn` rather than blocked on inline, so a slow write never
/// stalls the sandboxed call's own stdout-reading loop.
struct DbProgressSink {
    state: AppState,
    run_id: String,
    tenant_id: i64,
    last_write: Mutex<Option<Instant>>,
}

impl ProgressSink for DbProgressSink {
    fn report(&self, pct: Option<i64>, msg: Option<String>) {
        let now = Instant::now();
        {
            let mut last = self.last_write.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(prev) = *last
                && now.duration_since(prev) < PROGRESS_WRITE_MIN_INTERVAL
            {
                return;
            }
            *last = Some(now);
        }
        let progress_json = json!({"pct": pct, "msg": msg}).to_string();
        let state = self.state.clone();
        let run_id = self.run_id.clone();
        let tenant_id = self.tenant_id;
        tokio::spawn(async move {
            let _ = state.db.update_run_progress(run_id, tenant_id, progress_json).await;
        });
    }
}

/// The outcome `run_one_job` finalizes into a `runs` row.
enum JobOutcome {
    Done { result_value: Value },
    Error { error_class: String },
    Timeout,
}

/// P0 requirement 4: leases and runs a single job through the same `Kind`
/// dispatch path a synchronous call uses (schema already validated at
/// `enqueue` time, so this trusts the stored `args_json`), under
/// `deadline_s` from the plan rather than [`crate::state::CALL_TIMEOUT`].
async fn execute_job(state: &AppState, run: &RunRow, cancel_pid: CancelPidSlot) -> JobOutcome {
    let deadline_s = run.deadline_s.unwrap_or(300).max(1) as u64;

    let Ok(Some(tenant)) = state.db.find_tenant_by_id(run.tenant_id).await else {
        return JobOutcome::Error {
            error_class: "tenant_not_found".to_string(),
        };
    };
    let Ok(Some(row)) = state.db.get_tool(run.tenant_id, run.tool_name.clone()).await else {
        return JobOutcome::Error {
            error_class: "tool_not_found".to_string(),
        };
    };
    let Some(kind) = state.kinds.get(&row.kind) else {
        return JobOutcome::Error {
            error_class: "internal".to_string(),
        };
    };
    let args: Value = run
        .args_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| Value::Object(Default::default()));

    let Ok(secrets) = build_secret_resolver(state, tenant.id).await else {
        return JobOutcome::Error {
            error_class: "internal".to_string(),
        };
    };
    let log = Arc::new(BufferedLog(std::sync::Mutex::new(Vec::new())));
    let resources = Arc::new(CellResourceSink(std::sync::Mutex::new(None)));
    let progress: Arc<dyn ProgressSink> = Arc::new(DbProgressSink {
        state: state.clone(),
        run_id: run.id.clone(),
        tenant_id: run.tenant_id,
        last_write: Mutex::new(None),
    });
    let ctx = CallCtx {
        tenant_id: tenant.id,
        namespace: tenant.namespace.clone(),
        secrets,
        deadline: Instant::now() + Duration::from_secs(deadline_s),
        log: log.clone() as Arc<dyn CallLog>,
        test_mode: false,
        resources: resources.clone() as Arc<dyn ResourceSink>,
        tool_name: Some(run.tool_name.clone()),
        state: Arc::new(CountingStateBackend::new(
            Arc::new(TenantStateBridge {
                state: Arc::new(state.clone()),
                tenant: tenant.clone(),
            }),
            Some(log.clone() as Arc<dyn CallLog>),
        )),
        compose_depth: 0,
        compose_children: Some(Arc::new(std::sync::atomic::AtomicU32::new(0))),
        compose_db: Some(state.db.clone()),
        compose_kinds: Some(state.kinds.clone()),
        concurrent_calls_per_tenant: usize::MAX,
        run_id: Some(run.id.clone()),
        progress,
        cancel_pid: cancel_pid.clone(),
    };

    let outcome = tokio::time::timeout(
        Duration::from_secs(deadline_s),
        kind.call(&row.spec, args, &ctx),
    )
    .await;

    // PRD-mcphost-schedules P0 requirement 5: `host.tool_logs` lines from a
    // scheduled run carry `trigger_ref` (the trigger id) alongside
    // `run_id`, so an agent debugging a schedule can grep its own tool's
    // logs for just that trigger's firings.
    let log_prefix = match &run.trigger_ref {
        Some(trigger_ref) if run.trigger == "schedule" => {
            format!("run_id={} trigger_ref={trigger_ref}", run.id)
        }
        _ => format!("run_id={}", run.id),
    };
    for line in log.0.lock().map(|g| g.clone()).unwrap_or_default() {
        let _ = state
            .db
            .append_log(tenant.id, run.tool_name.clone(), format!("{log_prefix} {line}"))
            .await;
    }

    match outcome {
        Ok(Ok(value)) => JobOutcome::Done { result_value: value },
        Ok(Err(kind_err)) => {
            let app_err = AppError::from(kind_err);
            JobOutcome::Error {
                error_class: app_err.code().to_string(),
            }
        }
        Err(_elapsed) => {
            // AC2: the deadline passed without a response -- kill the
            // sandbox directly (the same pid `host.runs.cancel` would use)
            // rather than leaving it running unattended past its budget.
            if let Some(pid) = *cancel_pid.lock().unwrap_or_else(|e| e.into_inner()) {
                // `pid` is the same process-group leader pid
                // `host.runs.cancel` kills via `killpg` above.
                // SAFETY: killpg's only precondition is a valid pid; `pid`
                // SAFETY: is one by construction above.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            JobOutcome::Timeout
        }
    }
}

/// Runs one leased job to completion and finalizes its `runs` row. Never
/// panics out of the executor's own spawn -- every fallible step inside
/// `execute_job` degrades to a `JobOutcome::Error` instead.
async fn run_one_job(state: AppState, run: RunRow) {
    let run_id = run.id.clone();
    let tenant_id = run.tenant_id;
    let cancel_pid = state.runs.register(&run_id);
    let start = Instant::now();

    let outcome = execute_job(&state, &run, cancel_pid).await;
    state.runs.unregister(&run_id);

    let finished_unix = crate::state::now_unix();
    let duration_ms = start.elapsed().as_millis() as i64;

    let (status, result_ref, error_class) = match outcome {
        JobOutcome::Done { result_value } => {
            // P0 requirement 6: the result lives in the tenant's state
            // store under `runs/<run_id>`, bounded by the same quota
            // (`state_bytes_max`) and size cap (`MAX_TOOL_OUTPUT_BYTES`)
            // any other state write is -- `tenant_state::state_set` is the
            // one function that already enforces both, so this reuses it
            // rather than writing to `tenant_state_kv` directly.
            let result_key = format!("runs/{run_id}");
            match state.db.find_tenant_by_id(tenant_id).await {
                Ok(Some(tenant)) => {
                    let set_args = json!({"key": result_key, "value": result_value});
                    match crate::tenant_state::state_set(&state, &tenant, &set_args).await {
                        Ok(_) => ("done".to_string(), Some(result_key), None),
                        Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
                    }
                }
                _ => (
                    "error".to_string(),
                    None,
                    Some("tenant_not_found".to_string()),
                ),
            }
        }
        JobOutcome::Error { error_class } => ("error".to_string(), None, Some(error_class)),
        JobOutcome::Timeout => ("timeout".to_string(), None, None),
    };

    let _ = state
        .db
        .finalize_run(run_id, tenant_id, status, result_ref, error_class, finished_unix, duration_ms)
        .await;
}

/// One tick of the executor's leasing loop: admits at most one new job per
/// tenant with room (per-tenant `jobs_concurrent`, and the host-wide
/// [`JOBS_HOST_CEILING`]) -- one lease per tenant per tick, not "drain the
/// tenant's whole queue in one tick", so a single tenant flooding
/// `async=true` calls cannot starve the scan of every other tenant sharing
/// this tick.
async fn tick(state: &AppState) -> Result<(), AppError> {
    let running_total = state.db.count_running_runs_total().await? as usize;
    if running_total >= JOBS_HOST_CEILING {
        return Ok(());
    }
    let tenants = state.db.distinct_tenants_with_queued_runs().await?;
    for tenant_id in tenants {
        if state.db.count_running_runs_total().await? as usize >= JOBS_HOST_CEILING {
            break;
        }
        let Some(tenant) = state.db.find_tenant_by_id(tenant_id).await? else {
            continue;
        };
        let Some(plan) = state.plans.get(&tenant.plan) else {
            continue;
        };
        let running_for_tenant = state.db.count_running_runs_for_tenant(tenant_id).await?;
        if running_for_tenant >= plan.jobs_concurrent {
            continue;
        }
        if let Some(run) = state.db.lease_next_queued_run(tenant_id).await? {
            let state = state.clone();
            tokio::spawn(run_one_job(state, run));
        }
    }
    Ok(())
}

/// P0 requirement 4: "a tokio task beside the warm-pool reaper that leases
/// `queued` runs ... and finalizes the row." Started once at `mcphost
/// serve` startup (`main.rs`); never joined, same lifetime convention
/// `kinds::python::spawn_warm_reaper` already uses for its own background
/// task.
pub fn spawn_executor(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(EXECUTOR_TICK).await;
            if let Err(e) = tick(&state).await {
                tracing::warn!(error = %e, "runs executor tick failed");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `run_to_json`'s `purged` derivation (AC7): `purged_unix` present ->
    /// `purged: true`, regardless of whether `result_ref` is still set (it
    /// never is once purged, but this asserts the derivation reads the
    /// right column, not `result_ref`'s absence).
    #[test]
    fn run_to_json_reports_purged_from_purged_unix() {
        let run = RunRow {
            id: "01AAAA".to_string(),
            tenant_id: 1,
            tool_name: "transform".to_string(),
            trigger: "job".to_string(),
            trigger_ref: None,
            caller_tenant_id: None,
            status: "done".to_string(),
            progress_json: None,
            result_ref: None,
            error_class: None,
            started_unix: Some(100),
            finished_unix: Some(200),
            duration_ms: Some(100_000),
            deadline_s: Some(300),
            attempt: 1,
            purged_unix: Some(300),
            args_json: None,
            manual: false,
            test: false,
        };
        let value = run_to_json(&run);
        assert_eq!(value["purged"], json!(true));
        assert_eq!(value["result"], Value::Null);

        let not_purged = RunRow {
            purged_unix: None,
            ..run
        };
        assert_eq!(run_to_json(&not_purged)["purged"], json!(false));
    }
}
