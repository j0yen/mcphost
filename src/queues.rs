//! PRD-mcphost-queue-primitive-v2: `host.queue.create/push/stats/pause/resume/
//! drain/list/delete` and the dispatcher that turns a queue's pending items
//! into ordinary `runs` rows (`trigger = "queue"`), at most `concurrency` per
//! queue and never past the tenant's `jobs_concurrent`.
//!
//! The dispatcher (`spawn_dispatcher`) is a background tick beside
//! `runs::spawn_executor`; it only inserts `queued` runs, which the executor
//! leases as it does for any other trigger. Exactly-once start per item is the
//! claim `UPDATE` in `Db::queue_claim_and_start`, not `event_dedupe`.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::time::Duration;

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::db::queues::{Claim, QueueCounts, QueueCreate, QueueMove, QueuePush, QueueRow, QueueRun, QueueSpec};
use crate::errors::AppError;
use crate::state::AppState;

/// How often the dispatcher looks for deliverable items (R4: every second).
pub const DISPATCH_TICK: Duration = Duration::from_secs(1);

/// R3: the most items one `host.queue.push` takes.
pub const PUSH_MAX_ITEMS: usize = 1_000;

/// R2: a queue name.
pub const NAME_MAX_LEN: usize = 40;

crate::err_code! {
    pub QUEUE_NOT_FOUND,
    code: "queue_not_found", family: Storage, class: AgentFault, retryable: false,
    remedy: "List this tenant's queues with host.queue.list and use a name that exists.",
}

crate::err_code! {
    pub QUEUE_NOT_ACCEPTING,
    code: "queue_not_accepting", family: Call, class: HostRefusal, retryable: false,
    remedy: "The queue is draining or drained and takes no new items; create another queue to push to.",
}

crate::err_code! {
    pub QUEUE_ACTIVE,
    code: "queue_active", family: Call, class: HostRefusal, retryable: false,
    remedy: "Pause or drain the queue first, then delete it.",
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn plan_of<'a>(state: &'a AppState, tenant: &Tenant) -> Result<&'a crate::plans::Plan, AppError> {
    state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))
}

/// The schema pattern of a queue name (`host.queue.create`'s `name`), registered in
/// `host_args::SHAPE_CHECKS` beside [`is_valid_name`], the check it mirrors.
pub const NAME_PATTERN: &str = "^[a-z0-9_-]{1,40}$";

/// R2: a queue name matches [`NAME_PATTERN`].
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= NAME_MAX_LEN && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

async fn queue_named(state: &AppState, tenant: &Tenant, args: &Value) -> Result<QueueRow, AppError> {
    let name = arg_str(args, "name")?;
    state.db.queue_get(tenant.id, name.clone()).await?.ok_or_else(|| queue_not_found(&name))
}

fn queue_not_found(name: &str) -> AppError {
    AppError::Structured {
        code: QUEUE_NOT_FOUND,
        message: format!("no queue named '{name}' for this tenant"),
        data: json!({"name": name, "hint": "host.queue.list"}),
    }
}

fn counts_json(c: &QueueCounts) -> Value {
    json!({"pending": c.pending, "running": c.running, "done": c.done, "failed": c.failed})
}

fn stats_json(q: &QueueRow, c: &QueueCounts) -> Value {
    let now_ms = crate::state::now_unix_ms();
    json!({
        "name": q.name,
        "state": q.state,
        "consumer_tool": q.consumer_tool,
        "concurrency": q.concurrency,
        "pending": c.pending,
        "running": c.running,
        "done": c.done,
        "failed": c.failed,
        "attempts_total": c.attempts_total,
        "oldest_pending_age_ms": c.oldest_pending_ms.map(|t| (now_ms - t).max(0)),
        "created_at_ms": q.created_at_ms,
        "drained_at_ms": q.drained_at_ms,
    })
}

/// `host.queue.create {name, consumer_tool, concurrency}`.
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if !is_valid_name(&name) {
        return Err(AppError::InvalidArgs(format!("name '{name}' must match {NAME_PATTERN}")));
    }
    let consumer_tool = arg_str(args, "consumer_tool")?;
    let concurrency = match args.get("concurrency") {
        None | Some(Value::Null) => 1,
        Some(v) => v.as_i64().filter(|n| *n >= 1).ok_or_else(|| AppError::InvalidArgs("concurrency must be an integer of at least 1".to_string()))?,
    };
    let plan = plan_of(state, tenant)?;
    state
        .db
        .get_tool(tenant.id, consumer_tool.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(consumer_tool.clone()))?;
    if concurrency > plan.queue_concurrency_max {
        return Err(crate::billing::quota_exceeded(state, &tenant.plan, "queue_concurrency_max", plan.queue_concurrency_max, concurrency, None));
    }
    match state.db.queue_create(tenant.id, QueueSpec { name: name.clone(), consumer_tool, concurrency }, plan.queues_max).await? {
        QueueCreate::Created(q) => Ok(json!({
            "name": q.name,
            "state": q.state,
            "consumer_tool": q.consumer_tool,
            "concurrency": q.concurrency,
            "created_at_ms": q.created_at_ms,
        })),
        QueueCreate::Duplicate => Err(AppError::InvalidArgs(format!("a queue named '{name}' already exists for this tenant"))),
        QueueCreate::Quota { used } => Err(crate::billing::quota_exceeded(state, &tenant.plan, "queues_max", plan.queues_max, used, None)),
    }
}

/// `host.queue.push {name, items[]}`.
pub async fn push(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let queue = queue_named(state, tenant, args).await?;
    let items = args
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'items'".to_string()))?;
    if items.is_empty() || items.len() > PUSH_MAX_ITEMS {
        return Err(AppError::InvalidArgs(format!("items holds 1 to {PUSH_MAX_ITEMS} entries, got {}", items.len())));
    }
    let plan = plan_of(state, tenant)?;
    let (validator, _) = crate::runs::consumer_validator(state, tenant, &queue.consumer_tool).await?;
    let mut accepted: Vec<String> = Vec::with_capacity(items.len());
    let mut rejected: Vec<Value> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let encoded = serde_json::to_string(item).map_err(|e| AppError::Internal(format!("item serialize: {e}")))?;
        if encoded.len() as i64 > plan.queue_item_bytes_max {
            rejected.push(json!({
                "index": index,
                "reason": format!("item_too_large: {} bytes exceeds queue_item_bytes_max {}", encoded.len(), plan.queue_item_bytes_max),
            }));
            continue;
        }
        if let Some(v) = &validator
            && let Err(e) = v.validate(item)
        {
            rejected.push(json!({"index": index, "reason": format!("args_invalid: {e}")}));
            continue;
        }
        accepted.push(encoded);
    }
    let count = accepted.len();
    match state.db.queue_push(tenant.id, queue.id, accepted, plan.queue_items_max).await? {
        QueuePush::Pushed { first_idx, last_idx } => Ok(json!({
            "accepted": count,
            "rejected": rejected,
            "first_idx": first_idx,
            "last_idx": last_idx,
        })),
        QueuePush::NotAccepting(state_name) => Err(AppError::Structured {
            code: QUEUE_NOT_ACCEPTING,
            message: format!("queue '{}' is {state_name} and takes no new items", queue.name),
            data: json!({"name": queue.name, "state": state_name}),
        }),
        QueuePush::Quota { used } => Err(crate::billing::quota_exceeded(state, &tenant.plan, "queue_items_max", plan.queue_items_max, used, None)),
        QueuePush::Gone => Err(queue_not_found(&queue.name)),
    }
}

/// `host.queue.stats {name}`.
pub async fn stats(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    match state.db.queue_get_with_counts(tenant.id, name.clone()).await? {
        Some((q, c)) => Ok(stats_json(&q, &c)),
        None => Err(queue_not_found(&name)),
    }
}

/// `host.queue.list`.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.queue_list(tenant.id).await?;
    let queues: Vec<Value> = rows
        .iter()
        .map(|(q, c)| {
            let mut v = counts_json(c);
            v["name"] = json!(q.name);
            v["state"] = json!(q.state);
            v
        })
        .collect();
    Ok(json!({"queues": queues}))
}

/// What a state verb answers: the queue's stats after the move.
async fn moved(state: &AppState, tenant: &Tenant, name: &str, outcome: QueueMove, verb: &str) -> Result<Value, AppError> {
    match outcome {
        QueueMove::Moved(_) => match state.db.queue_get_with_counts(tenant.id, name.to_string()).await? {
            Some((q, c)) => Ok(stats_json(&q, &c)),
            None => Err(queue_not_found(name)),
        },
        QueueMove::Refused(q) => Err(AppError::InvalidArgs(format!("host.queue.{verb} applies to an active or paused queue; '{}' is {}", q.name, q.state))),
        QueueMove::Gone => Err(queue_not_found(name)),
    }
}

/// `host.queue.pause {name}`: `active` → `paused`.
pub async fn pause(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let outcome = state.db.queue_pause(tenant.id, name.clone()).await?;
    moved(state, tenant, &name, outcome, "pause").await
}

/// `host.queue.resume {name}`: `paused` → `active`.
pub async fn resume(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let outcome = state.db.queue_resume(tenant.id, name.clone()).await?;
    moved(state, tenant, &name, outcome, "resume").await
}

/// `host.queue.drain {name}`: `active|paused` → `draining` (→ `drained` once empty).
pub async fn drain(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let outcome = state.db.queue_drain(tenant.id, name.clone()).await?;
    moved(state, tenant, &name, outcome, "drain").await
}

/// `host.queue.delete {name}`: a `drained` or `paused` queue and its items.
pub async fn delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    match state.db.queue_delete(tenant.id, name.clone()).await? {
        QueueMove::Moved(q) => Ok(json!({"name": q.name, "deleted": true})),
        QueueMove::Refused(q) => Err(AppError::Structured {
            code: QUEUE_ACTIVE,
            message: format!("queue '{}' is {}; pause or drain first", q.name, q.state),
            data: json!({"name": q.name, "state": q.state, "hint": "pause or drain first"}),
        }),
        QueueMove::Gone => Err(queue_not_found(&name)),
    }
}

// ---- dispatcher ----

/// One dispatcher tick: settle finished items, then start pending items of every
/// deliverable queue (`active` and `draining`) in `idx` order, as far as the
/// queue's `concurrency` and the tenant's `jobs_concurrent` leave room.
pub async fn tick(state: &AppState) -> Result<(), AppError> {
    state.db.queue_reconcile().await?;
    let mut tenants: HashMap<i64, Option<(Tenant, i64)>> = HashMap::new();
    for (queue, items) in state.db.queue_dispatch_candidates().await? {
        let known = match tenants.entry(queue.tenant_id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
                let found = state.db.find_tenant_by_id(queue.tenant_id).await?.filter(|t| !t.disabled);
                v.insert(found.and_then(|t| state.plans.get(&t.plan).map(|p| p.jobs_concurrent).map(|j| (t, j))))
            }
        };
        let Some((tenant, jobs_concurrent)) = known.as_ref() else {
            continue;
        };
        for (item_id, item_json) in items {
            let args: Value = match serde_json::from_str(&item_json) {
                Ok(v) => v,
                Err(e) => {
                    state.db.queue_item_fail(item_id, format!("args_invalid: {e}")).await?;
                    continue;
                }
            };
            let deadline_s = match crate::runs::consumer_validator(state, tenant, &queue.consumer_tool).await {
                Ok((validator, deadline_s)) => match validator.as_ref().and_then(|v| v.iter_errors(&args).next()) {
                    Some(e) => {
                        state.db.queue_item_fail(item_id, format!("args_invalid: {e}")).await?;
                        continue;
                    }
                    None => deadline_s,
                },
                Err(AppError::ToolNotFound(_)) => {
                    state.db.queue_item_fail(item_id, crate::errors::TOOL_NOT_FOUND.to_string()).await?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let run = QueueRun {
                run_id: crate::state::new_ulid(),
                tool_name: queue.consumer_tool.clone(),
                args_json: item_json,
                deadline_s,
            };
            match state.db.queue_claim_and_start(item_id, queue.clone(), *jobs_concurrent, run).await? {
                Claim::Started | Claim::Lost => {}
                Claim::NoRoom => break,
            }
        }
    }
    Ok(())
}

/// Started once at `mcphost serve` startup, beside `runs::spawn_executor`
/// (R4); never joined. Rollback (PRD § Migration): stop spawning this -- rows stay.
pub fn spawn_dispatcher(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(DISPATCH_TICK).await;
            if let Err(e) = tick(&state).await {
                tracing::warn!(error = %e, "queue dispatcher tick failed");
            }
        }
    })
}
