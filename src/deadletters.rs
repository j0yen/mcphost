//! PRD-mcphost-event-trigger-from-webhook: every refused `POST /hooks/...`
//! delivery and every event-fired run that finalized `error` is a dead
//! letter the tenant can list (`host.trigger.dead_letters`) and replay
//! (`host.trigger.replay(dead_letter_id)`).

use axum::http::HeaderMap;
use serde_json::{Value, json};

use crate::db::{DeadLetterRow, NewDeadLetter, TriggerRow};
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};

/// `dead_letter_id` prefix, so `host.trigger.replay` can tell a letter id
/// from a run id.
pub const ID_PREFIX: &str = "dl_";

/// `host.trigger.dead_letters`'s `limit` ceiling.
pub const LIST_LIMIT_MAX: i64 = 200;
const LIST_LIMIT_DEFAULT: i64 = 50;

/// How many characters of a body `body_excerpt` shows.
const EXCERPT_CHARS: usize = 200;

/// One reason a delivery or run became a dead letter.
pub struct DeadLetterReason {
    /// The `reason_code` stored on a letter.
    pub code: &'static str,
    /// The `AppError` code (or run `error_class`) the hook path raises for
    /// this reason; `""` when the reason is not an error code (`run_error`
    /// carries the run's own `error_class`).
    pub error_code: &'static str,
    /// Whether a letter is written for it (`false`: the refusal happens
    /// before any trigger is resolved, so there is nothing to attach to).
    pub stored: bool,
    pub description: &'static str,
}

/// The one table of dead-letter reasons: the hook refusal path, the tool
/// description and the drift test all read it.
pub const DEAD_LETTER_REASONS: &[DeadLetterReason] = &[
    DeadLetterReason {
        code: EVENTS_RATE_LIMITED,
        error_code: EVENTS_RATE_LIMITED,
        stored: true,
        description: "more deliveries than the plan's events_per_minute in the last minute (HTTP 429)",
    },
    DeadLetterReason {
        code: EVENT_BODY_TOO_LARGE,
        error_code: EVENT_BODY_TOO_LARGE,
        stored: true,
        description: "body over the plan's event_body_bytes_max (HTTP 413); body is cut to that size",
    },
    DeadLetterReason {
        code: SIGNATURE_MISMATCH,
        error_code: crate::hooks::SIGNATURE_INVALID,
        stored: true,
        description: "the verify config rejected the signature, token or timestamp (HTTP 401)",
    },
    DeadLetterReason {
        code: SECRET_UNAVAILABLE,
        error_code: crate::errors::SECRET_MISSING,
        stored: true,
        description: "the verify config names a secret this tenant no longer has (HTTP 400)",
    },
    DeadLetterReason {
        code: TOOL_MISSING,
        error_code: HOOK_NOT_FOUND,
        stored: true,
        description: "the trigger exists but its target tool was removed (HTTP 404)",
    },
    DeadLetterReason {
        code: JOBS_CONCURRENT,
        error_code: JOBS_CONCURRENT_EXCEEDED,
        stored: true,
        description: "the tenant was at its jobs_concurrent ceiling; the run was rejected (HTTP 202)",
    },
    DeadLetterReason {
        code: RUN_ERROR,
        error_code: "",
        stored: true,
        description: "the event-fired run finalized error; the letter carries run_id and error_class",
    },
    DeadLetterReason {
        code: HOOK_NOT_FOUND,
        error_code: HOOK_NOT_FOUND,
        stored: false,
        description: "no enabled event trigger resolved for the URL (HTTP 404); no letter, nothing to attach it to",
    },
    DeadLetterReason {
        code: BANNED,
        error_code: BANNED,
        stored: false,
        description: "the owning key is banned (HTTP 403); no letter",
    },
];

/// The reason a hook-path `AppError` code is stored under.
pub fn reason_for_error(error_code: &str) -> Option<&'static DeadLetterReason> {
    DEAD_LETTER_REASONS
        .iter()
        .find(|r| r.stored && r.error_code == error_code)
}

/// `host.trigger.dead_letters`'s tool description, rendered from
/// [`DEAD_LETTER_REASONS`] so it can never name a code the table lacks.
pub fn tool_description() -> String {
    let codes: Vec<String> = DEAD_LETTER_REASONS
        .iter()
        .map(|r| format!("{} ({})", r.code, r.description))
        .collect();
    format!(
        "List this tenant's dead letters, newest first: every event delivery the host refused \
         and every event-fired run that ended in error, with reason_code, received_unix, \
         body_excerpt and dead_letter_id; dropped_count is how many the ring evicted. Replay \
         one with host.trigger.replay(dead_letter_id). reason_code is one of: {}.",
        codes.join("; ")
    )
}

fn truncate_chars(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Everything a refusal knows when it writes a letter.
pub struct LetterSource<'a> {
    pub reason_code: &'a str,
    pub headers: Value,
    pub body: &'a [u8],
    pub run_id: Option<&'a str>,
    pub error_class: Option<&'a str>,
}

/// Stores one letter for `trigger`, returning its `dead_letter_id`. The body
/// is cut to the plan's `event_body_bytes_max`; the tenant's ring is trimmed
/// to the plan's `dead_letters_max`.
pub async fn write(
    state: &AppState,
    tenant_id: i64,
    plan: &crate::plans::Plan,
    trigger: &TriggerRow,
    src: LetterSource<'_>,
) -> Result<String, AppError> {
    let plan_body_max = plan.event_body_bytes_max;
    let id = format!("{ID_PREFIX}{}", new_ulid());
    let text = String::from_utf8_lossy(src.body);
    let (body, truncated) = truncate_chars(&text, plan_body_max.max(0) as usize);
    state
        .db
        .insert_dead_letter(
            NewDeadLetter {
                id: id.clone(),
                tenant_id,
                trigger_id: trigger.id.clone(),
                received_unix: now_unix(),
                reason_code: src.reason_code.to_string(),
                headers_json: src.headers.to_string(),
                body,
                truncated,
                run_id: src.run_id.map(str::to_string),
                error_class: src.error_class.map(str::to_string),
            },
            plan.dead_letters_max,
        )
        .await?;
    Ok(id)
}

/// A letter as `host.trigger.dead_letters` shows it.
pub fn letter_json(row: &DeadLetterRow) -> Value {
    let headers: Value = serde_json::from_str(&row.headers_json).unwrap_or_else(|_| json!({}));
    let excerpt: String = row.body.chars().take(EXCERPT_CHARS).collect();
    json!({
        "dead_letter_id": row.id,
        "trigger_id": row.trigger_id,
        "received_unix": row.received_unix,
        "reason_code": row.reason_code,
        "headers_json": headers,
        "body": row.body,
        "body_excerpt": excerpt,
        "truncated": row.truncated,
        "run_id": row.run_id,
        "replayed_run_id": row.replayed_run_id,
        "error_class": row.error_class,
    })
}

/// A redacted header map for a stored letter (the allow list is the
/// trigger's own, same as an event's run args).
pub fn redacted(headers: &HeaderMap, allow: &[String]) -> Value {
    crate::hooks::redacted_headers_json(headers, allow)
}

/// `host.trigger.dead_letters(trigger?, limit?, since?)`.
pub async fn list(
    state: &AppState,
    tenant: &crate::db::Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let trigger_id = match args.get("trigger").and_then(Value::as_str) {
        Some(name_or_id) => Some(resolve_trigger(state, tenant, name_or_id).await?),
        None => None,
    };
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(LIST_LIMIT_DEFAULT)
        .clamp(1, LIST_LIMIT_MAX);
    let since = args.get("since").and_then(Value::as_i64);
    let rows = state
        .db
        .list_dead_letters(tenant.id, trigger_id, since, limit)
        .await?;
    let letters: Vec<Value> = rows.iter().map(letter_json).collect();
    Ok(json!({
        "dead_letters": letters,
        "limit": limit,
        "dropped_count": state.db.dead_letters_dropped(tenant.id).await?,
    }))
}

/// A trigger named by id or by name; unknown -> `trigger_not_found`.
async fn resolve_trigger(
    state: &AppState,
    tenant: &crate::db::Tenant,
    name_or_id: &str,
) -> Result<String, AppError> {
    if let Some(row) = state.db.get_trigger(tenant.id, name_or_id.to_string()).await? {
        return Ok(row.id);
    }
    crate::triggers::resolve(state, tenant, &json!({"name": name_or_id}))
        .await
        .map(|r| r.id)
}

/// An event-fired run finalized `error`: stores a `run_error` letter holding
/// the delivery the run was enqueued with. A run whose `trigger_ref` is not a
/// live trigger of the tenant (a run-id replay) has nothing to attach to and
/// writes none. Best effort -- never fails the run.
pub async fn record_run_error(
    state: &AppState,
    run: &crate::db::RunRow,
    error_class: Option<&str>,
) {
    if run.trigger != "event" {
        return;
    }
    let Some(trigger_ref) = run.trigger_ref.clone() else {
        return;
    };
    let Ok(Some(trigger)) = state.db.get_trigger(run.tenant_id, trigger_ref).await else {
        return;
    };
    let Ok(Some(tenant)) = state.db.find_tenant_by_id(run.tenant_id).await else {
        return;
    };
    let Some(plan) = state.plans.get(&tenant.plan) else {
        return;
    };
    let args: Value = run
        .args_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let event = args.get("event").cloned().unwrap_or(Value::Null);
    let body = match event.get("body") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let headers = event.get("headers").cloned().unwrap_or_else(|| json!({}));
    let written = write(
        state,
        run.tenant_id,
        plan,
        &trigger,
        LetterSource {
            reason_code: RUN_ERROR,
            headers,
            body: body.as_bytes(),
            run_id: Some(&run.id),
            error_class,
        },
    )
    .await;
    if let Err(e) = written {
        tracing::warn!(error = %e, run_id = %run.id, "failed to write run_error dead letter");
    }
}

/// `host.trigger.get`'s `dead_letters: {count, newest_unix}`.
pub async fn stats_json(state: &AppState, tenant_id: i64, trigger_id: &str) -> Value {
    match state.db.dead_letter_stats(tenant_id, trigger_id.to_string()).await {
        Ok((count, newest)) => json!({"count": count, "newest_unix": newest}),
        Err(_) => json!({"count": 0, "newest_unix": null}),
    }
}

crate::err_code! {
    pub BANNED,
    code: "banned", family: Auth, class: HostRefusal, retryable: false,
    remedy: "This key is banned; do not retry. Contact support through the support_url on /support.html.",
}

crate::err_code! {
    pub EVENT_BODY_TOO_LARGE,
    code: "event_body_too_large", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Send a smaller event body; the limit is the plan's event_body_bytes_max.",
}

crate::err_code! {
    pub EVENTS_RATE_LIMITED,
    code: "events_rate_limited", family: Plan, class: PlanLimit, retryable: true,
    remedy: "Slow the delivery rate and retry; the plan caps events_per_minute.",
}

crate::err_code! {
    pub HOOK_NOT_FOUND,
    code: "hook_not_found", family: Storage, class: AgentFault, retryable: false,
    remedy: "Create the event trigger for this tool first, or use one that exists.",
}

crate::err_code! {
    pub JOBS_CONCURRENT,
    code: "jobs_concurrent", family: Plan, class: PlanLimit, retryable: true,
    remedy: "Wait for a running job to finish; the tenant is at its jobs_concurrent ceiling.",
}

crate::err_code! {
    pub JOBS_CONCURRENT_EXCEEDED,
    code: "jobs_concurrent_exceeded", family: Plan, class: PlanLimit, retryable: true,
    remedy: "Wait for a running job to finish; the tenant is at its jobs_concurrent ceiling.",
}

crate::err_code! {
    pub RUN_ERROR,
    code: "run_error", family: Call, class: AgentFault, retryable: false,
    remedy: "Read the run's error_class and fix the tool.",
}

crate::err_code! {
    pub SECRET_UNAVAILABLE,
    code: "secret_unavailable", family: Call, class: AgentFault, retryable: false,
    remedy: "Set the missing secret with host.secret.set, then retry.",
}

crate::err_code! {
    pub SIGNATURE_MISMATCH,
    code: "signature_mismatch", family: Auth, class: AgentFault, retryable: false,
    remedy: "Re-send with a signature that matches the verify config.",
}

crate::err_code! {
    pub TOOL_MISSING,
    code: "tool_missing", family: Storage, class: AgentFault, retryable: false,
    remedy: "Restore the target tool or remove the trigger.",
}

