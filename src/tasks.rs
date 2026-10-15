//! PRD-mcphost-task-handoff-tool: `host.task.*`, a task as a thread with a
//! state machine the host owns. Delivery rides the existing thread/inbox
//! path (`messaging::send`/`reply`), so `host.msg.wait`, `host.msg.inbox`,
//! message triggers and the mesh ops see a task message unchanged -- the
//! `data.task` key is the only new shape.

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

/// R2: `deadline_s` bounds (seconds).
pub const DEADLINE_S_MIN: i64 = 60;
pub const DEADLINE_S_MAX: i64 = 86_400;
/// R5: the deadline sweep's cadence; a dead-lettered task is at most this
/// late (the PRD allows 60 s).
pub const SWEEP_EVERY_S: u64 = 30;
/// R5: `host.task.dead_letters` looks back this far, returning at most
/// [`DEAD_LETTERS_MAX`] rows.
pub const DEAD_LETTERS_WINDOW_S: i64 = 7 * 86_400;
pub const DEAD_LETTERS_MAX: i64 = 100;
/// R2: a `dedupe_key` is honoured for this long after the original post.
pub const DEDUPE_WINDOW_S: i64 = 86_400;

/// R1: the task state vocabulary. Add a variant here and to [`Self::ALL`];
/// the schema enum, the descriptions and llms.txt follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Posted,
    Claimed,
    Done,
    Dead,
    Cancelled,
}

impl TaskStatus {
    /// The one source every consumer reads.
    pub const ALL: [TaskStatus; 5] = [
        TaskStatus::Posted,
        TaskStatus::Claimed,
        TaskStatus::Done,
        TaskStatus::Dead,
        TaskStatus::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Posted => "posted",
            TaskStatus::Claimed => "claimed",
            TaskStatus::Done => "done",
            TaskStatus::Dead => "dead",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<TaskStatus> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The JSON-schema `enum` array for every `status` the host.task.*
    /// output schemas declare.
    pub fn schema_enum() -> Value {
        Value::Array(Self::ALL.iter().map(|k| json!(k.as_str())).collect())
    }

    /// The words, as the one line `www/llms.txt` carries (backticked,
    /// comma-separated) and the `host.task.get` description repeats.
    pub fn llms_txt_statuses() -> String {
        Self::ALL.iter().map(|k| format!("`{}`", k.as_str())).collect::<Vec<_>>().join(", ")
    }
}

/// The host.task.* tools whose output schema carries a `status` property.
pub const STATUS_TOOLS: &[&str] = &["host.task.post", "host.task.claim", "host.task.complete", "host.task.get"];

/// One `tasks` row.
#[derive(Debug, Clone)]
pub struct TaskRow {
    pub id: String,
    pub thread_id: Option<String>,
    pub poster_tenant_id: i64,
    pub recipient_tenant_id: i64,
    pub status: String,
    pub payload: Value,
    pub result: Option<Value>,
    pub created_unix: i64,
    pub deadline_unix: i64,
    pub claimed_unix: Option<i64>,
    pub done_unix: Option<i64>,
    pub dead_unix: Option<i64>,
    pub dead_reason: Option<String>,
}

const TASK_COLUMNS: &str = "id, thread_id, poster_tenant_id, recipient_tenant_id, status, payload_json, \
    result_json, created_unix, deadline_unix, claimed_unix, done_unix, dead_unix, dead_reason";

fn task_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    let payload: String = r.get(5)?;
    let result: Option<String> = r.get(6)?;
    Ok(TaskRow {
        id: r.get(0)?,
        thread_id: r.get(1)?,
        poster_tenant_id: r.get(2)?,
        recipient_tenant_id: r.get(3)?,
        status: r.get(4)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        result: result.and_then(|s| serde_json::from_str(&s).ok()),
        created_unix: r.get(7)?,
        deadline_unix: r.get(8)?,
        claimed_unix: r.get(9)?,
        done_unix: r.get(10)?,
        dead_unix: r.get(11)?,
        dead_reason: r.get(12)?,
    })
}

/// R3: byte-identical for an unknown id and for a caller who is not a party
/// to the task.
pub fn task_not_found() -> AppError {
    AppError::Structured {
        code: "task_not_found",
        message: "no task found for that id".to_string(),
        data: json!({}),
    }
}

fn task_already_claimed(task_id: &str) -> AppError {
    AppError::Structured {
        code: "task_already_claimed",
        message: "that task is already claimed".to_string(),
        data: json!({"task_id": task_id, "status": TaskStatus::Claimed.as_str()}),
    }
}

fn task_not_claimed(task_id: &str) -> AppError {
    AppError::Structured {
        code: "task_not_claimed",
        message: "claim the task before completing it".to_string(),
        data: json!({"task_id": task_id, "status": TaskStatus::Posted.as_str(), "hint": "host.task.claim"}),
    }
}

fn task_closed(task_id: &str, status: &str) -> AppError {
    AppError::Structured {
        code: "task_closed",
        message: format!("that task is {status} and takes no further claim or completion"),
        data: json!({"task_id": task_id, "status": status}),
    }
}

fn recipient_inbox_full() -> AppError {
    AppError::Structured {
        code: "recipient_inbox_full",
        message: "that agent's inbox is full".to_string(),
        data: json!({}),
    }
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_object(args: &Value, name: &str) -> Result<Value, AppError> {
    match args.get(name) {
        Some(v @ Value::Object(_)) => Ok(v.clone()),
        Some(_) => Err(AppError::InvalidArgs(format!("{name}: must be a JSON object"))),
        None => Err(AppError::InvalidArgs(format!("missing required argument '{name}'"))),
    }
}

async fn load_task(state: &AppState, task_id: &str, tenant_id: i64) -> Result<Option<TaskRow>, AppError> {
    let task_id = task_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.query_row(
                &format!(
                    "SELECT {TASK_COLUMNS} FROM tasks \
                     WHERE id = ?1 AND (poster_tenant_id = ?2 OR recipient_tenant_id = ?2)"
                ),
                params![task_id, tenant_id],
                task_from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
}

/// Called by `messaging::send_outcome` once the carrying message has
/// committed and before any message trigger fires.
pub(crate) async fn link_thread(state: &AppState, task_id: &str, thread_id: &str) -> Result<(), AppError> {
    let (task_id, thread_id) = (task_id.to_string(), thread_id.to_string());
    state
        .db
        .with_conn(move |conn| {
            conn.execute("UPDATE tasks SET thread_id = ?2 WHERE id = ?1", params![task_id, thread_id])?;
            Ok(())
        })
        .await
}

fn task_view(t: &TaskRow) -> Value {
    json!({
        "task_id": t.id,
        "thread_id": t.thread_id,
        "status": t.status,
        "deadline_unix": t.deadline_unix,
    })
}

/// `host.task.post(to, payload, deadline_s, dedupe_key?)` (R2 / AC1).
pub async fn post(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let to = arg_str(args, "to")?;
    let payload = arg_object(args, "payload")?;
    let deadline_s = match args.get("deadline_s").and_then(Value::as_i64) {
        Some(n) if (DEADLINE_S_MIN..=DEADLINE_S_MAX).contains(&n) => n,
        _ => {
            return Err(AppError::InvalidArgs(format!(
                "deadline_s: required integer from {DEADLINE_S_MIN} to {DEADLINE_S_MAX}"
            )));
        }
    };
    let dedupe_key = args.get("dedupe_key").and_then(Value::as_str).map(str::to_string);
    let now = crate::state::now_unix();

    if let Some(key) = &dedupe_key {
        let (poster, key, since) = (tenant.id, key.clone(), now - DEDUPE_WINDOW_S);
        let hit = state
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    &format!(
                        "SELECT {TASK_COLUMNS} FROM tasks \
                         WHERE poster_tenant_id = ?1 AND dedupe_key = ?2 AND created_unix >= ?3"
                    ),
                    params![poster, key, since],
                    task_from_row,
                )
                .optional()
                .map_err(AppError::from)
            })
            .await?;
        if let Some(t) = hit {
            return Ok(task_view(&t));
        }
    }

    // R6: open tasks per poster (`posted|claimed`).
    let plan = state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))?;
    let open = count_open(state, tenant.id).await?;
    if open >= plan.tasks_open_max {
        return Err(AppError::msg_quota_exceeded("tasks_open_max", plan.tasks_open_max));
    }

    let recipient_id = state
        .db
        .resolve_agent_address(to.clone())
        .await?
        .ok_or_else(AppError::agent_not_found)?;

    let task_id = crate::state::new_ulid();
    let deadline_unix = now + deadline_s;
    {
        let (task_id, payload_json, dedupe_key, poster) =
            (task_id.clone(), payload.to_string(), dedupe_key.clone(), tenant.id);
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO tasks (id, poster_tenant_id, recipient_tenant_id, status, payload_json, \
                     created_unix, deadline_unix, dedupe_key) VALUES (?1, ?2, ?3, 'posted', ?4, ?5, ?6, ?7)",
                    params![task_id, poster, recipient_id, payload_json, now, deadline_unix, dedupe_key],
                )?;
                Ok(())
            })
            .await?;
    }

    let send_args = json!({
        "to": [to],
        "body": format!("task {task_id} posted"),
        "data": {
            "task": {"task_id": task_id, "status": TaskStatus::Posted.as_str(), "deadline_unix": deadline_unix},
            "payload": payload,
        },
    });
    let outcome = match crate::messaging::send_outcome(state, tenant, &send_args, Some(&task_id)).await {
        Ok(o) => o,
        Err(e) => {
            delete_task(state, &task_id).await;
            return Err(e);
        }
    };
    if let Some((_, code)) = outcome.refused.first() {
        delete_task(state, &task_id).await;
        return Err(match *code {
            "contact_refused" => AppError::contact_refused(),
            "recipient_inbox_full" => recipient_inbox_full(),
            _ => AppError::agent_not_found(),
        });
    }
    Ok(json!({
        "task_id": task_id,
        "thread_id": outcome.thread_id,
        "status": TaskStatus::Posted.as_str(),
        "deadline_unix": deadline_unix,
    }))
}

async fn count_open(state: &AppState, poster: i64) -> Result<i64, AppError> {
    state
        .db
        .with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE poster_tenant_id = ?1 AND status IN ('posted', 'claimed')",
                params![poster],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
}

async fn delete_task(state: &AppState, task_id: &str) {
    let task_id = task_id.to_string();
    let _ = state
        .db
        .with_conn(move |conn| {
            conn.execute("DELETE FROM tasks WHERE id = ?1", params![task_id])?;
            Ok(())
        })
        .await;
}

/// Posts a task-state reply from `tenant` into the task's thread.
async fn reply_state(
    state: &AppState,
    tenant: &Tenant,
    task: &TaskRow,
    status: TaskStatus,
    result: Option<&Value>,
) -> Result<(), AppError> {
    let Some(thread_id) = &task.thread_id else { return Ok(()) };
    let mut t = json!({"task_id": task.id, "status": status.as_str()});
    if let Some(r) = result {
        t["result"] = r.clone();
    }
    let reply_args = json!({
        "thread_id": thread_id,
        "body": format!("task {} {}", task.id, status.as_str()),
        "data": {"task": t},
    });
    crate::messaging::reply_outcome(state, tenant, &reply_args).await.map(|_| ())
}

/// `host.task.claim(task_id)` (R3 / AC3): the recipient only.
pub async fn claim(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let task_id = arg_str(args, "task_id")?;
    let now = crate::state::now_unix();
    let (id, recipient) = (task_id.clone(), tenant.id);
    let changed = state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE tasks SET status = 'claimed', claimed_unix = ?3 \
                 WHERE id = ?1 AND recipient_tenant_id = ?2 AND status = 'posted'",
                params![id, recipient, now],
            )
            .map_err(AppError::from)
        })
        .await?;
    let row = load_task(state, &task_id, tenant.id)
        .await?
        .filter(|t| t.recipient_tenant_id == tenant.id)
        .ok_or_else(task_not_found)?;
    if changed == 0 {
        return Err(match TaskStatus::parse(&row.status) {
            Some(TaskStatus::Claimed) => task_already_claimed(&task_id),
            _ => task_closed(&task_id, &row.status),
        });
    }
    // The poster's wait wakes on the claim; a failed notice never undoes it.
    if let Err(e) = reply_state(state, tenant, &row, TaskStatus::Claimed, None).await {
        tracing::warn!(error = %e, task_id = %task_id, "task claim notice failed");
    }
    Ok(json!({
        "task_id": row.id,
        "thread_id": row.thread_id,
        "status": TaskStatus::Claimed.as_str(),
        "payload": row.payload,
        "deadline_unix": row.deadline_unix,
    }))
}

/// `host.task.complete(task_id, result)` (R4 / AC1, AC5): the claimer only;
/// idempotent on a done task.
pub async fn complete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let task_id = arg_str(args, "task_id")?;
    let result = arg_object(args, "result")?;
    let now = crate::state::now_unix();
    let (id, recipient, result_json) = (task_id.clone(), tenant.id, result.to_string());
    let changed = state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE tasks SET status = 'done', result_json = ?3, done_unix = ?4 \
                 WHERE id = ?1 AND recipient_tenant_id = ?2 AND status = 'claimed'",
                params![id, recipient, result_json, now],
            )
            .map_err(AppError::from)
        })
        .await?;
    let row = load_task(state, &task_id, tenant.id)
        .await?
        .filter(|t| t.recipient_tenant_id == tenant.id)
        .ok_or_else(task_not_found)?;
    if changed == 0 {
        return match TaskStatus::parse(&row.status) {
            Some(TaskStatus::Done) => Ok(json!({
                "task_id": row.id,
                "thread_id": row.thread_id,
                "status": TaskStatus::Done.as_str(),
                "result": row.result,
                "already_complete": true,
            })),
            Some(TaskStatus::Posted) => Err(task_not_claimed(&task_id)),
            _ => Err(task_closed(&task_id, &row.status)),
        };
    }
    if let Err(e) = reply_state(state, tenant, &row, TaskStatus::Done, Some(&result)).await {
        // No completion without its message: put the claim back.
        let (id, recipient) = (task_id.clone(), tenant.id);
        let _ = state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE tasks SET status = 'claimed', result_json = NULL, done_unix = NULL \
                     WHERE id = ?1 AND recipient_tenant_id = ?2 AND status = 'done'",
                    params![id, recipient],
                )?;
                Ok(())
            })
            .await;
        return Err(e);
    }
    Ok(json!({
        "task_id": row.id,
        "thread_id": row.thread_id,
        "status": TaskStatus::Done.as_str(),
        "result": result,
        "already_complete": false,
    }))
}

/// `host.task.get(task_id)` (R1 / AC2, AC7): either party reads the task.
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let task_id = arg_str(args, "task_id")?;
    let row = load_task(state, &task_id, tenant.id).await?.ok_or_else(task_not_found)?;
    Ok(json!({
        "task_id": row.id,
        "thread_id": row.thread_id,
        "status": row.status,
        "role": if row.poster_tenant_id == tenant.id { "poster" } else { "recipient" },
        "payload": row.payload,
        "result": row.result,
        "reason": row.dead_reason,
        "created_unix": row.created_unix,
        "deadline_unix": row.deadline_unix,
        "claimed_unix": row.claimed_unix,
        "done_unix": row.done_unix,
        "dead_unix": row.dead_unix,
    }))
}

/// R5: every `posted|claimed` task whose deadline is at or before `now_unix`
/// becomes `dead` (`reason: "deadline"`) and the thread gets a `task.dead`
/// message from `host`, delivered to both parties. Exposed with an explicit
/// clock so a test drives one tick deterministically. Returns the count.
pub async fn sweep_at(state: &AppState, now_unix: i64) -> Result<usize, AppError> {
    state
        .db
        .with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let swept: Result<usize, AppError> = (|| {
                let due: Vec<(String, Option<String>, i64, i64)> = {
                    let mut stmt = conn.prepare(
                        "SELECT id, thread_id, poster_tenant_id, recipient_tenant_id FROM tasks \
                         WHERE status IN ('posted', 'claimed') AND deadline_unix <= ?1",
                    )?;
                    stmt.query_map(params![now_unix], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for (task_id, thread_id, poster, recipient) in &due {
                    conn.execute(
                        "UPDATE tasks SET status = 'dead', dead_unix = ?2, dead_reason = 'deadline' WHERE id = ?1",
                        params![task_id, now_unix],
                    )?;
                    if let Some(thread_id) = thread_id {
                        insert_dead_message(conn, thread_id, task_id, &[*poster, *recipient], now_unix)?;
                    }
                }
                Ok(due.len())
            })();
            match &swept {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ())?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            swept
        })
        .await
}

/// The `task.dead` thread message: `from_tenant_id` NULL and `from_address`
/// `host`, the same shape the contact-request notice uses.
fn insert_dead_message(
    conn: &rusqlite::Connection,
    thread_id: &str,
    task_id: &str,
    recipients: &[i64],
    now_unix: i64,
) -> Result<(), AppError> {
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE thread_id = ?1",
        params![thread_id],
        |r| r.get(0),
    )?;
    let message_id = crate::state::new_ulid();
    let data = json!({"task": {"task_id": task_id, "status": TaskStatus::Dead.as_str(), "reason": "deadline"}});
    let now_ms = crate::state::now_unix_ms().max(now_unix.saturating_mul(1000));
    conn.execute(
        "INSERT INTO messages \
             (id, thread_id, seq, from_tenant_id, from_address, body, data_json, in_reply_to, \
              dedupe_key, synthetic, source_class, created_at, created_unix_ms, refused_json, urgent) \
         VALUES (?1, ?2, ?3, NULL, 'host', ?4, ?5, NULL, NULL, NULL, 'host', ?6, ?7, NULL, 0)",
        params![
            message_id,
            thread_id,
            seq,
            format!("task {task_id} dead: deadline passed"),
            data.to_string(),
            crate::db::now_rfc3339(),
            now_ms,
        ],
    )?;
    for tenant_id in recipients {
        conn.execute(
            "INSERT OR IGNORE INTO message_receipts (message_id, tenant_id, read_at) VALUES (?1, ?2, NULL)",
            params![message_id, tenant_id],
        )?;
    }
    Ok(())
}

/// [`sweep_at`] at the wall clock.
pub async fn sweep(state: &AppState) -> Result<usize, AppError> {
    sweep_at(state, crate::state::now_unix()).await
}

/// Started once at `mcphost serve` startup (`main.rs`), same convention as
/// [`crate::channels::spawn_channel_retention`]; never joined.
pub fn spawn_sweep(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SWEEP_EVERY_S)).await;
            if let Err(e) = sweep(&state).await {
                tracing::warn!(error = %e, "task deadline sweep failed");
            }
        }
    })
}

/// `host.task.dead_letters()` (R5 / AC4): the caller's own dead tasks from
/// the last 7 days, newest first, at most 100.
pub async fn dead_letters(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let (poster, since) = (tenant.id, crate::state::now_unix() - DEAD_LETTERS_WINDOW_S);
    let rows = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks \
                 WHERE poster_tenant_id = ?1 AND status = 'dead' AND dead_unix >= ?2 \
                 ORDER BY dead_unix DESC, id DESC LIMIT {DEAD_LETTERS_MAX}"
            ))?;
            stmt.query_map(params![poster, since], task_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(AppError::from)
        })
        .await?;
    let letters: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({
                "task_id": t.id,
                "thread_id": t.thread_id,
                "status": t.status,
                "reason": t.dead_reason,
                "payload": t.payload,
                "deadline_unix": t.deadline_unix,
                "dead_unix": t.dead_unix,
            })
        })
        .collect();
    Ok(json!({"dead_letters": letters, "limit": DEAD_LETTERS_MAX}))
}
