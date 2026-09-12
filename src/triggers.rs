//! PRD-mcphost-schedules: the `triggers` table's tenant/admin-facing
//! business logic (`host.trigger.*`, `admin.triggers`) plus the scheduler
//! tick that enqueues a run when a schedule is due. Same
//! `AppState`/arguments-in, `serde_json::Value`(or [`AppError`])-out
//! convention as `runs.rs`/`control.rs`/`tenant_state.rs` -- `handler.rs`
//! is the only place that touches `rmcp` wire types.
//!
//! The tick (`spawn_scheduler`) is the one background task besides
//! `kinds::python`'s warm-pool reaper and `runs::spawn_executor` -- it
//! shares nothing with either except the general "a tokio task started
//! once at `mcphost serve` startup" shape (technical considerations: "the
//! tick, the reaper and the sandbox recheck can share one supervisor task
//! so `/healthz` reports them together" is satisfied by all three being
//! independent, equally-cheap loops rather than one literal task, since
//! none of the three has any state the others need).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use serde_json::{Value, json};

use crate::cron::CronSchedule;
use crate::db::{Tenant, TriggerRow};
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};

/// P0 requirement 3: the scheduler's own polling interval -- coarser than
/// [`crate::runs::EXECUTOR_TICK`] since a schedule's own granularity is a
/// whole minute at best (`cron`'s non-goal: "sub-minute schedules"), so
/// 30s comfortably keeps firing lateness under the success metric's p95
/// 45s bound without polling any faster than the thing it's polling for
/// can even change.
const SCHEDULER_TICK: Duration = Duration::from_secs(30);

/// P0 requirement 5: `/healthz`'s `scheduler_last_tick_unix` -- the
/// scheduler's own liveness, read the same way [`crate::runs::RunsRegistry`]
/// is: an `Arc<Mutex<..>>` field on [`AppState`], written by the tick loop,
/// read by `http.rs`.
#[derive(Clone, Default)]
pub struct SchedulerStatus {
    last_tick_unix: Arc<Mutex<Option<i64>>>,
}

impl SchedulerStatus {
    pub fn new() -> Self {
        Self::default()
    }

    fn record_tick(&self, unix: i64) {
        *self.last_tick_unix.lock().unwrap_or_else(|e| e.into_inner()) = Some(unix);
    }

    /// `None` until the first tick has actually run (a box mid-startup,
    /// before `spawn_scheduler`'s first iteration).
    pub fn last_tick_unix(&self) -> Option<i64> {
        *self.last_tick_unix.lock().unwrap_or_else(|e| e.into_inner())
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

/// `pub(crate)`: `hooks.rs` builds the same `trigger_invalid` code for its
/// own event-trigger validation (AC4's "verify" rejections), rather than a
/// second constructor for one error code.
pub(crate) fn trigger_invalid(field: &'static str, message: String) -> AppError {
    AppError::Structured {
        code: "trigger_invalid",
        message,
        data: json!({"field": field}),
    }
}

/// `pub(crate)`: `hooks.rs`'s `host.trigger.test` reuses this for an
/// unknown trigger id, same code/shape a schedule trigger's own
/// pause/resume/remove already return.
pub(crate) fn trigger_not_found(id: &str) -> AppError {
    AppError::Structured {
        code: "trigger_not_found",
        message: format!("no trigger '{id}' for this tenant"),
        data: json!({"id": id}),
    }
}

/// The stored shape of a schedule trigger's `config_json` -- `args` is
/// always present (defaults to `{}`), `tz` only when the caller set one
/// (P1 requirement 6; see [`validate_tz`]'s doc comment for why anything
/// but `"UTC"` is refused for now).
fn build_config(schedule: &str, args: &Value, tz: Option<&str>) -> Value {
    let mut config = json!({"schedule": schedule, "args": args});
    if let (Some(tz), Some(obj)) = (tz, config.as_object_mut()) {
        obj.insert("tz".to_string(), json!(tz));
    }
    config
}

/// `pub(crate)`: `hooks.rs`'s own `set_event_trigger` reuses this rather
/// than duplicating a second SHA-256-hex helper, since it's the same
/// "content hash backing a `triggers` UNIQUE constraint" purpose for either
/// kind's `config_json`.
pub(crate) fn config_hash(config_json: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config_json.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Parses a trigger row's `config_json` back into `(schedule_expr, args,
/// tz)`. `args`/`tz` degrade to `{}`/`None` on a malformed stored value
/// (should never happen -- this crate is the only writer) rather than
/// failing the caller's read.
fn parse_stored_config(config_json: &str) -> (String, Value, Option<String>) {
    let config: Value = serde_json::from_str(config_json).unwrap_or_else(|_| json!({}));
    let schedule = config
        .get("schedule")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let args = config.get("args").cloned().unwrap_or_else(|| json!({}));
    let tz = config.get("tz").and_then(Value::as_str).map(str::to_string);
    (schedule, args, tz)
}

/// P1 requirement 6 / AC9: `tz` beyond UTC needs an IANA tzdata table
/// (DST transitions, historical offset changes) this crate has no
/// dependency for -- the PRD's own non-goals list "timezones beyond UTC"
/// as P1, so rather than silently computing `next_unix` in UTC while
/// claiming a `tz` was honored, an explicit non-UTC `tz` is refused with
/// `trigger_invalid` naming `tz`. `"UTC"` (case-insensitive) is accepted
/// as a no-op, since that's what every schedule already computes in.
fn validate_tz(tz: &str) -> Result<(), AppError> {
    if tz.eq_ignore_ascii_case("utc") {
        Ok(())
    } else {
        Err(trigger_invalid(
            "tz",
            format!(
                "tz: '{tz}' is not supported yet -- only \"UTC\" (or omitting tz) works in this \
                 version; IANA timezones are P1, not yet built"
            ),
        ))
    }
}

fn trigger_base_json(row: &TriggerRow, last_status: Option<String>) -> Value {
    json!({
        "id": row.id,
        "tool": row.tool_name,
        "kind": row.kind,
        "enabled": row.enabled,
        "created_unix": row.created_unix,
        "next_unix": row.next_unix,
        "last_run_id": row.last_run_id,
        "last_fired_unix": row.last_fired_unix,
        "last_status": last_status,
    })
}

/// Builds the tenant-facing JSON for one trigger row, reading its own
/// `last_run_id`'s current status (if any) so `host.trigger.list`/`get`'s
/// `last_status` is always fresh rather than a snapshot from firing time.
/// PRD-mcphost-inbound-events: an event-kind row's shape (`url`, `verify`,
/// `unverified`, `dedupe_header` instead of `schedule`/`tz`) is different
/// enough that it's built in `hooks.rs`, which owns everything else
/// event-specific too.
async fn trigger_to_json(state: &AppState, tenant: &Tenant, row: TriggerRow) -> Value {
    if row.kind == "event" {
        return crate::hooks::trigger_to_json_event(state, tenant, &row).await;
    }
    let last_status = match &row.last_run_id {
        Some(run_id) => state
            .db
            .get_run(run_id.clone(), tenant.id)
            .await
            .ok()
            .flatten()
            .map(|r| r.status),
        None => None,
    };
    let (schedule, args, tz) = parse_stored_config(&row.config_json);
    let mut value = trigger_base_json(&row, last_status);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("schedule".to_string(), json!(schedule));
        obj.insert("args".to_string(), args);
        obj.insert("tz".to_string(), json!(tz));
    }
    value
}

/// `host.trigger.set(tool, kind="schedule"|"event", ...)` (P0 requirement
/// 2; PRD-mcphost-inbound-events P0 requirement 3 added `kind="event"`).
/// The tool lookup is shared by both kinds; each kind's own argument shape
/// and quota lives in [`set_schedule`]/[`crate::hooks::set_event_trigger`].
pub async fn set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let tool = arg_str(args, "tool")?;
    let kind = arg_str_opt(args, "kind").unwrap_or_else(|| "schedule".to_string());
    state
        .db
        .get_tool(tenant.id, tool.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(tool.clone()))?;

    match kind.as_str() {
        "schedule" => set_schedule(state, tenant, &tool, args).await,
        // PRD-mcphost-inbound-events P0 requirement 3: the second trigger
        // kind, goal 3's "an inbound event later" finally wired up. Its own
        // verify-config validation, quota and URL-building live in
        // `hooks.rs` (that module also owns `POST /hooks/...` itself), not
        // here -- this function stays the one place that decides which
        // kind an argument shape belongs to.
        "event" => crate::hooks::set_event_trigger(state, tenant, &tool, args).await,
        other => Err(trigger_invalid(
            "kind",
            format!(
                "kind: '{other}' is not supported yet -- only \"schedule\" and \"event\" work in \
                 this version"
            ),
        )),
    }
}

async fn set_schedule(
    state: &AppState,
    tenant: &Tenant,
    tool: &str,
    args: &Value,
) -> Result<Value, AppError> {
    let schedule_expr = arg_str(args, "schedule")?;
    let call_args = args.get("args").cloned().unwrap_or_else(|| json!({}));
    let tz = arg_str_opt(args, "tz");
    if let Some(tz) = &tz {
        validate_tz(tz)?;
    }

    let schedule = CronSchedule::parse(&schedule_expr).map_err(cron_error_to_app_error)?;

    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;

    let now = now_unix();
    let next1 = schedule.next_after(now).ok_or_else(|| {
        trigger_invalid(
            "schedule",
            "schedule: this expression never fires (e.g. a day-of-month/month combination \
             that never occurs)"
                .to_string(),
        )
    })?;
    // P0 requirement 4: the actual gap between this schedule's own next two
    // occurrences, not a guess from the cron text -- honest for any field
    // combination, not just the simple `*/N` case.
    if let Some(next2) = schedule.next_after(next1) {
        let interval = next2 - next1;
        if interval < plan.schedule_min_interval_s {
            return Err(AppError::Structured {
                code: "trigger_interval_too_short",
                message: format!(
                    "schedule fires every {interval}s, under the plan's \
                     {}s minimum",
                    plan.schedule_min_interval_s
                ),
                data: json!({"schedule_min_interval_s": plan.schedule_min_interval_s}),
            });
        }
    }

    let existing = state.db.count_schedule_triggers_for_tenant(tenant.id).await?;
    if existing >= plan.schedules_max {
        return Err(AppError::Structured {
            code: "trigger_quota_exceeded",
            message: format!(
                "tenant already holds {existing} schedules, the plan maximum of {}",
                plan.schedules_max
            ),
            data: json!({"schedules_max": plan.schedules_max}),
        });
    }

    let config = build_config(&schedule_expr, &call_args, tz.as_deref());
    let config_json = serde_json::to_string(&config)
        .map_err(|e| AppError::Internal(format!("trigger config serialize: {e}")))?;
    let hash = config_hash(&config_json);
    let id = new_ulid();
    state
        .db
        .insert_trigger(
            id.clone(),
            tenant.id,
            tool.to_string(),
            "schedule".to_string(),
            config_json,
            hash,
            Some(next1),
        )
        .await
        .map_err(|_| {
            trigger_invalid(
                "schedule",
                "schedule: this tool already has an identical schedule trigger".to_string(),
            )
        })?;

    let row = state
        .db
        .get_trigger(tenant.id, id.clone())
        .await?
        .ok_or_else(|| AppError::Internal("trigger vanished immediately after insert".to_string()))?;
    Ok(trigger_to_json(state, tenant, row).await)
}

/// [`CronSchedule::parse`]'s [`crate::cron::CronError`] -> [`AppError`],
/// naming the exact field the PRD's AC2 requires.
fn cron_error_to_app_error(e: crate::cron::CronError) -> AppError {
    AppError::Structured {
        code: "trigger_invalid",
        message: e.message,
        data: json!({"field": e.field}),
    }
}

/// `host.trigger.list(tool?)`.
pub async fn list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let tool = arg_str_opt(args, "tool");
    let rows = state.db.list_triggers(tenant.id, tool).await?;
    let mut triggers = Vec::with_capacity(rows.len());
    for row in rows {
        triggers.push(trigger_to_json(state, tenant, row).await);
    }
    Ok(json!({"triggers": triggers}))
}

/// `host.trigger.get(id)`.
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let row = state
        .db
        .get_trigger(tenant.id, id.clone())
        .await?
        .ok_or_else(|| trigger_not_found(&id))?;
    Ok(trigger_to_json(state, tenant, row).await)
}

async fn set_enabled(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    enabled: bool,
) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let ok = state
        .db
        .set_trigger_enabled(tenant.id, id.clone(), enabled)
        .await?;
    if !ok {
        return Err(trigger_not_found(&id));
    }
    Ok(json!({"id": id, "enabled": enabled}))
}

/// `host.trigger.pause(id)`.
pub async fn pause(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    set_enabled(state, tenant, args, false).await
}

/// `host.trigger.resume(id)` (AC6): leaves `next_unix` exactly as it was --
/// if time already passed while paused, the very next tick treats it as
/// due and fires once (the non-goal "a missed firing is recorded, not
/// replayed" falls out of this for free: there was only ever one
/// `next_unix` to miss, never a backlog).
pub async fn resume(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    set_enabled(state, tenant, args, true).await
}

/// `host.trigger.remove(id)` (AC7's per-trigger primitive; `host.tool_remove`
/// disables rather than removes, see [`crate::db::Db::disable_triggers_for_tool`]).
pub async fn remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let ok = state.db.remove_trigger(tenant.id, id.clone()).await?;
    if !ok {
        return Err(trigger_not_found(&id));
    }
    Ok(json!({"removed": id}))
}

/// `host.trigger.fire(id)` (P1 requirement 7 / AC10): runs the schedule
/// once right now, independent of `next_unix`/`enabled` -- a paused
/// trigger can still be fired manually for testing.
pub async fn fire(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let row = state
        .db
        .get_trigger(tenant.id, id.clone())
        .await?
        .ok_or_else(|| trigger_not_found(&id))?;
    let (_, call_args, _) = parse_stored_config(&row.config_json);
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let args_json = serde_json::to_string(&call_args)
        .map_err(|e| AppError::Internal(format!("trigger args serialize: {e}")))?;
    let run_id = new_ulid();
    state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            row.tool_name.clone(),
            "schedule".to_string(),
            Some(row.id.clone()),
            None,
            plan.job_max_s,
            args_json,
            true,
            false,
        )
        .await?;
    Ok(json!({"run_id": run_id, "status": "queued", "manual": true}))
}

/// `admin.triggers(tenant?, kind?)`.
pub async fn admin_triggers(state: &AppState, args: &Value) -> Result<Value, AppError> {
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
    let kind = arg_str_opt(args, "kind");
    let rows = state.db.admin_list_triggers(tenant_id, kind).await?;
    let triggers: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            // PRD-mcphost-inbound-events: an event row's `config_json` has
            // no `schedule`/`tz` -- `parse_stored_config` degrades those to
            // `""`/`None` harmlessly, but the admin view should say `url`
            // instead of a misleadingly-empty `schedule`.
            let mut value = if row.kind == "event" {
                // No tenant namespace on hand at this cross-tenant admin
                // listing without an extra lookup per row -- `verify`
                // (never the secret's plaintext, only its name) is the
                // useful admin-facing fact; an operator wanting the exact
                // URL already knows the namespace and can compute it.
                let config: Value = serde_json::from_str(&row.config_json).unwrap_or_else(|_| json!({}));
                json!({"verify": config.get("verify").cloned().unwrap_or(Value::Null)})
            } else {
                let (schedule, _args, tz) = parse_stored_config(&row.config_json);
                json!({"schedule": schedule, "tz": tz})
            };
            if let Some(obj) = value.as_object_mut() {
                obj.insert("id".to_string(), json!(row.id));
                obj.insert("tenant_id".to_string(), json!(row.tenant_id));
                obj.insert("tool".to_string(), json!(row.tool_name));
                obj.insert("kind".to_string(), json!(row.kind));
                obj.insert("enabled".to_string(), json!(row.enabled));
                obj.insert("next_unix".to_string(), json!(row.next_unix));
                obj.insert("last_run_id".to_string(), json!(row.last_run_id));
                obj.insert("last_fired_unix".to_string(), json!(row.last_fired_unix));
            }
            value
        })
        .collect();
    Ok(json!({"triggers": triggers}))
}

/// One scheduler tick (P0 requirement 3): every due, enabled schedule
/// trigger either enqueues a run (advancing `next_unix` to the following
/// occurrence) or, if its previous run is still `queued`/`running`, is
/// recorded as a `skipped`/`overlap` run instead -- the safe default the
/// PRD's technical considerations name, so an overrunning monitor never
/// piles up concurrent executions of itself. Exposed `pub` (rather than
/// only reachable through [`spawn_scheduler`]'s own 30s loop) so
/// integration tests can drive one tick deterministically instead of
/// waiting on real wall-clock cron alignment.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let now = now_unix();
    let due = state.db.due_schedule_triggers(now).await?;
    for row in due {
        let overlapping = matches!(
            state.db.last_run_status_for_trigger(row.id.clone()).await?.as_deref(),
            Some("queued") | Some("running")
        );
        let (schedule_expr, call_args, _tz) = parse_stored_config(&row.config_json);
        let next_unix = CronSchedule::parse(&schedule_expr)
            .ok()
            .and_then(|s| s.next_after(now));

        if overlapping {
            let run_id = new_ulid();
            state
                .db
                .insert_skipped_run(run_id, row.tenant_id, row.tool_name.clone(), row.id.clone())
                .await?;
            state
                .db
                .update_trigger_after_fire(row.id.clone(), next_unix, None, now)
                .await?;
            continue;
        }

        let Some(tenant) = state.db.find_tenant_by_id(row.tenant_id).await? else {
            // Tenant deleted out from under a still-enabled trigger should
            // be impossible (cascade delete removes the trigger row too --
            // migration 0015's `ON DELETE CASCADE`), but skip rather than
            // panic if it ever happens.
            continue;
        };
        let Some(plan) = state.plans.get(&tenant.plan) else {
            continue;
        };
        let args_json = serde_json::to_string(&call_args).unwrap_or_else(|_| "{}".to_string());
        let run_id = new_ulid();
        state
            .db
            .insert_queued_run(
                run_id.clone(),
                row.tenant_id,
                row.tool_name.clone(),
                "schedule".to_string(),
                Some(row.id.clone()),
                None,
                plan.job_max_s,
                args_json,
                false,
                false,
            )
            .await?;
        state
            .db
            .update_trigger_after_fire(row.id.clone(), next_unix, Some(run_id), now)
            .await?;
    }
    state.scheduler.record_tick(now);
    Ok(())
}

/// P0 requirement 3: "a tokio task beside the warm-pool reaper" -- started
/// once at `mcphost serve` startup (`main.rs`); never joined, same
/// lifetime convention `kinds::python::spawn_warm_reaper` and
/// `runs::spawn_executor` already use for their own background tasks.
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "scheduler tick failed");
            }
            tokio::time::sleep(SCHEDULER_TICK).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_hash_is_stable_and_content_sensitive() {
        let a = config_hash(r#"{"schedule":"* * * * *","args":{}}"#);
        let b = config_hash(r#"{"schedule":"* * * * *","args":{}}"#);
        let c = config_hash(r#"{"schedule":"*/5 * * * *","args":{}}"#);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn validate_tz_accepts_only_utc() {
        assert!(validate_tz("UTC").is_ok());
        assert!(validate_tz("utc").is_ok());
        assert!(validate_tz("America/Los_Angeles").is_err());
    }

    #[test]
    fn parse_stored_config_round_trips() {
        let config = build_config("*/5 * * * *", &json!({"n": 1}), Some("UTC"));
        let config_json = serde_json::to_string(&config).unwrap();
        let (schedule, args, tz) = parse_stored_config(&config_json);
        assert_eq!(schedule, "*/5 * * * *");
        assert_eq!(args, json!({"n": 1}));
        assert_eq!(tz.as_deref(), Some("UTC"));
    }
}
