//! Business logic for `host.msg.*` (PRD-mcphost-agent-inbox): directed
//! messages and threads between mcphost tenants, addressed through the
//! agent directory (`agents.rs`/migration 0020). Same split as
//! `agents.rs`/`control.rs`: pure `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out -- `handler.rs` is the only
//! place that touches `rmcp` wire types.
//!
//! Storage lives in `db.rs`'s `threads`/`thread_participants`/`messages`/
//! `message_receipts`/`blocks` (migration 0021); this module owns argument
//! validation, the four whole-call quota checks (requirement 7), and
//! turning `db::SendOutcome`/`db::MessageRow` into wire JSON.
//!
//! Cursor shape: `db.rs`'s technical considerations describe a base64
//! `(created_unix_ms, id)` pair; this crate carries no base64 dependency
//! (`agents::search`'s own cursor is a plain decimal offset string, not
//! base64 either), so both cursors here are plain, undelimited-by-dots
//! strings a caller must treat as opaque without actually needing a new
//! dependency to produce one: `host.msg.inbox`'s is `"<unix_ms>.<id>"`
//! (a ULID `id` never contains `.`); `host.msg.thread`'s is just the
//! decimal `seq` to resume after.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::db::{MessageRow, SendOutcome, Tenant};
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

/// Requirement 7: `msg_body_bytes_max` covers `body` plus `data_json`
/// combined (technical considerations: "16 KiB (body plus data_json)").
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

fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// requirement 12: `data_json` must be a JSON object when present; `body`
/// must be non-empty after trim.
fn validate_body_and_data(args: &Value) -> Result<(String, Option<String>), AppError> {
    let body = arg_str(args, "body")?;
    if body.trim().is_empty() {
        return Err(AppError::InvalidArgs("body: must be non-empty".to_string()));
    }
    let data_json = match args.get("data") {
        None | Some(Value::Null) => None,
        Some(v @ Value::Object(_)) => Some(v.to_string()),
        Some(_) => {
            return Err(AppError::InvalidArgs(
                "data: must be a JSON object".to_string(),
            ));
        }
    };
    Ok((body, data_json))
}

fn plan_for<'a>(state: &'a AppState, tenant: &Tenant) -> Result<&'a Plan, AppError> {
    state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))
}

/// requirement 6: `from` is server-set, never a caller argument -- rejected
/// as `args_invalid` before anything else runs (AC7).
fn reject_forged_from(args: &Value) -> Result<(), AppError> {
    if args.get("from").is_some() {
        return Err(AppError::InvalidArgs(
            "from: is set by the host from the authenticated tenant, not a caller argument"
                .to_string(),
        ));
    }
    Ok(())
}

fn send_outcome_json(outcome: SendOutcome) -> Value {
    json!({
        "message_id": outcome.message_id,
        "thread_id": outcome.thread_id,
        "seq": outcome.seq,
        "delivered_to": outcome.delivered_to,
        "refused": outcome.refused.into_iter()
            .map(|(address, code)| {
                // PRD-mcphost-agent-consent requirement 4 (AC1): the hint
                // is a fixed presentation of the code, not a stored fact,
                // so it's attached here at the wire-JSON layer rather than
                // threaded through `db.rs`'s stored/dedup `refused` shape
                // (see `AppError::contact_refused`'s doc comment for the
                // same reasoning applied to the top-level error this same
                // code can also surface as, from `host.agent.contact_request`).
                if code == "contact_refused" {
                    json!({"address": address, "code": code, "data": {"hint": "host.agent.contact_request"}})
                } else {
                    json!({"address": address, "code": code})
                }
            })
            .collect::<Vec<_>>(),
    })
}

/// `host.msg.send(to, body, data?, dedupe_key?, thread_id?)` (requirements
/// 2, 6, 8, 11 / AC1, AC4, AC5, AC7, AC9, AC10, AC14).
pub async fn send(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    reject_forged_from(args)?;
    let plan = plan_for(state, tenant)?;
    let (body, data_json) = validate_body_and_data(args)?;
    let dedupe_key = arg_str_opt(args, "dedupe_key");
    let thread_id = arg_str_opt(args, "thread_id");
    // PRD-mcphost-agent-consent requirement 6: default false, so every
    // pre-existing caller that never passes it keeps sending ordinary
    // (non-urgent) messages.
    let urgent = arg_bool(args, "urgent");

    let to: Vec<String> = match args.get("to") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| AppError::InvalidArgs("to: every entry must be a string".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(AppError::InvalidArgs("to: must be an array of addresses".to_string())),
        None => return Err(AppError::InvalidArgs("missing required argument 'to'".to_string())),
    };
    if to.is_empty() {
        return Err(AppError::InvalidArgs("to: must name at least one address".to_string()));
    }

    // requirement 7: whole-call quota checks, before anything is stored
    // (AC9/AC10: "stores nothing").
    let body_bytes = body.len() + data_json.as_ref().map(String::len).unwrap_or(0);
    if body_bytes > plan.msg_body_bytes_max as usize {
        return Err(AppError::msg_quota_exceeded("msg_body_bytes_max", plan.msg_body_bytes_max));
    }
    let recipient_cap_used = match &thread_id {
        // requirement 11 (AC14): cap counts total participants, not just
        // this call's new addresses.
        Some(t) => state.db.count_thread_participants(t.clone()).await? + to.len() as i64,
        None => to.len() as i64,
    };
    if recipient_cap_used > plan.recipients_per_msg_max {
        return Err(AppError::msg_quota_exceeded(
            "recipients_per_msg_max",
            plan.recipients_per_msg_max,
        ));
    }
    let now_ms = crate::state::now_unix_ms();
    let sent_this_hour = state.db.count_messages_sent_since(tenant.id, now_ms - 3_600_000).await?;
    if sent_this_hour >= plan.msgs_per_hour {
        return Err(AppError::msg_quota_exceeded("msgs_per_hour", plan.msgs_per_hour));
    }

    let outcome = state
        .db
        .msg_send(
            tenant.clone(),
            tenant.synthetic.clone(),
            to,
            thread_id,
            body.clone(),
            data_json.clone(),
            dedupe_key,
            plan.inbox_rows_max,
            urgent,
            plan.urgent_per_day,
        )
        .await?;

    // PRD-mcphost-agent-wake requirement 4: strictly after the message's
    // own insert has committed above, never inside that transaction.
    fire_message_triggers(
        state,
        &outcome,
        &MessageFireCtx {
            from_address: &tenant.namespace,
            body: &body,
            data_json: &data_json,
            in_reply_to: None,
        },
    )
    .await;

    Ok(send_outcome_json(outcome))
}

/// `host.msg.reply(thread_id, body, data?, in_reply_to?, dedupe_key?)`
/// (requirement 3 / AC2, AC3).
pub async fn reply(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    reject_forged_from(args)?;
    let plan = plan_for(state, tenant)?;
    let (body, data_json) = validate_body_and_data(args)?;
    let thread_id = arg_str(args, "thread_id")?;
    let in_reply_to = arg_str_opt(args, "in_reply_to");
    let dedupe_key = arg_str_opt(args, "dedupe_key");

    let body_bytes = body.len() + data_json.as_ref().map(String::len).unwrap_or(0);
    if body_bytes > plan.msg_body_bytes_max as usize {
        return Err(AppError::msg_quota_exceeded("msg_body_bytes_max", plan.msg_body_bytes_max));
    }
    let now_ms = crate::state::now_unix_ms();
    let sent_this_hour = state.db.count_messages_sent_since(tenant.id, now_ms - 3_600_000).await?;
    if sent_this_hour >= plan.msgs_per_hour {
        return Err(AppError::msg_quota_exceeded("msgs_per_hour", plan.msgs_per_hour));
    }

    let outcome = state
        .db
        .msg_reply(
            tenant.clone(),
            tenant.synthetic.clone(),
            thread_id,
            body.clone(),
            data_json.clone(),
            in_reply_to.clone(),
            dedupe_key,
            plan.inbox_rows_max,
        )
        .await?;

    // PRD-mcphost-agent-wake requirement 4: strictly after the message's
    // own insert has committed above, never inside that transaction.
    fire_message_triggers(
        state,
        &outcome,
        &MessageFireCtx {
            from_address: &tenant.namespace,
            body: &body,
            data_json: &data_json,
            in_reply_to: in_reply_to.as_deref(),
        },
    )
    .await;

    Ok(send_outcome_json(outcome))
}

/// PRD-mcphost-agent-wake requirements 2/3/4: after [`send`]/[`reply`]'s
/// own message insert has committed, fires every enabled `message`-kind
/// trigger each delivered recipient holds whose `from` is unset or matches
/// the sender -- one run per matching trigger, via the same
/// `hooks::enqueue_with_dedupe` helper `handle_hook` uses for an event
/// trigger's own delivery (requirement 4's explicit "factor ... not
/// copy-pasting it"). Never fails the send/reply that reached here: a
/// per-trigger enqueue failure is only logged (`enqueue_with_dedupe`
/// itself already degrades a quota rejection into a `rejected` run rather
/// than an `Err`; a genuine DB error is the only way this loop's own call
/// returns `Err`, and even that must not undo an already-durable message).
///
/// Envelope privacy (technical considerations): exactly `{message_id,
/// thread_id, seq, from, body, data, in_reply_to, created_at}` -- no block
/// list, no receipts, no other participant's address.
///
/// Bundled (rather than four positional arguments after `outcome`) to keep
/// `from_address`/`body` -- two adjacent `&str` fields -- transposition-
/// proof at each of the two call sites ([`send`]/[`reply`]), and to stay
/// under the crate's `too-many-arguments-threshold = 5` (`clippy.toml`).
struct MessageFireCtx<'a> {
    from_address: &'a str,
    body: &'a str,
    data_json: &'a Option<String>,
    in_reply_to: Option<&'a str>,
}

async fn fire_message_triggers(state: &AppState, outcome: &SendOutcome, ctx: &MessageFireCtx<'_>) {
    let from_address = ctx.from_address;
    let body = ctx.body;
    let data_json = ctx.data_json;
    let in_reply_to = ctx.in_reply_to;
    if outcome.delivered_tenant_ids.is_empty() {
        return;
    }
    let data = data_json.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok());
    let envelope = json!({
        "message_id": outcome.message_id,
        "thread_id": outcome.thread_id,
        "seq": outcome.seq,
        "from": from_address,
        "body": body,
        "data": data,
        "in_reply_to": in_reply_to,
        "created_at": outcome.created_at,
    });
    let args_json = envelope.to_string();

    for &recipient_id in &outcome.delivered_tenant_ids {
        let triggers = match state.db.list_enabled_message_triggers(recipient_id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = recipient_id, "message trigger lookup failed");
                continue;
            }
        };
        if triggers.is_empty() {
            continue;
        }
        let recipient = match state.db.find_tenant_by_id(recipient_id).await {
            Ok(Some(t)) => t,
            _ => continue,
        };
        for row in triggers {
            if let Some(from_scope) = crate::triggers::parse_message_trigger_from(&row.config_json)
                && from_scope != from_address
            {
                continue;
            }
            if let Err(e) = crate::hooks::enqueue_with_dedupe(
                state,
                &recipient,
                &row,
                crate::hooks::EnqueueSpec {
                    trigger_kind: "message",
                    dedupe_key: Some(outcome.message_id.as_str()),
                    message_id: Some(outcome.message_id.as_str()),
                    args_json: args_json.clone(),
                },
            )
            .await
            {
                tracing::warn!(error = %e, trigger_id = %row.id, "message trigger enqueue failed");
            }
        }
    }
}

fn encode_inbox_cursor(row: &MessageRow) -> String {
    format!("{}.{}", row.created_unix_ms, row.id)
}

fn decode_inbox_cursor(cursor: &str) -> Result<(i64, String), AppError> {
    let (ms, id) = cursor
        .split_once('.')
        .ok_or_else(|| AppError::InvalidArgs("cursor: malformed".to_string()))?;
    let ms: i64 = ms
        .parse()
        .map_err(|_| AppError::InvalidArgs("cursor: malformed".to_string()))?;
    Ok((ms, id.to_string()))
}

fn message_row_json(row: &MessageRow) -> Value {
    json!({
        "message_id": row.id,
        "thread_id": row.thread_id,
        "seq": row.seq,
        "from_address": row.from_address,
        "body": row.body,
        "data": row.data,
        "in_reply_to": row.in_reply_to,
        "synthetic": row.synthetic,
        "source_class": row.source_class,
        "created_at": row.created_at,
        "read_at": row.read_at,
        "urgent": row.urgent,
    })
}

/// `host.msg.inbox(cursor?, limit≤100, unread_only?)` (requirement 4, 5 /
/// AC6, AC13).
pub async fn inbox(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let limit = arg_i64_opt(args, "limit").unwrap_or(50).clamp(1, 100);
    let unread_only = arg_bool(args, "unread_only");
    let after = match arg_str_opt(args, "cursor") {
        Some(c) => Some(decode_inbox_cursor(&c)?),
        None => None,
    };

    let rows = state.db.msg_inbox(tenant.id, after, limit, unread_only).await?;
    let next_cursor = if rows.len() as i64 == limit {
        rows.last().map(encode_inbox_cursor)
    } else {
        None
    };
    Ok(json!({
        "messages": rows.iter().map(message_row_json).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    }))
}

/// PRD-mcphost-agent-wake P0 requirement 6: `host.msg.wait(cursor?,
/// timeout_s<=25, unread_only?)` -- for a client with no polling loop of
/// its own, same motivating shape `runs::wait` already proves for
/// `host.runs.wait` (500ms polls up to `timeout_s`; a real wake channel is
/// P1 requirement 9, out of scope here, same as it is for `runs::wait`'s
/// own P1). Returns as soon as at least one message past `cursor` exists
/// (the same shape [`inbox`] would for that call, but with its own
/// `next_cursor` always advanced to the last row returned, not gated on a
/// full page -- a caller polling forward wants "resume after what I just
/// got", not inbox's own pagination semantics), else at the deadline an
/// empty list with `cursor` unchanged (AC6). Never errors past argument
/// validation -- a bad `cursor` still fails fast, same as `inbox` itself.
pub async fn wait(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let timeout_s = arg_i64_opt(args, "timeout_s").unwrap_or(20).clamp(1, 25);
    let unread_only = arg_bool(args, "unread_only");
    let cursor_arg = arg_str_opt(args, "cursor");
    let after = match &cursor_arg {
        Some(c) => Some(decode_inbox_cursor(c)?),
        None => None,
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_s as u64);
    loop {
        let rows = state.db.msg_inbox(tenant.id, after.clone(), 50, unread_only).await?;
        if !rows.is_empty() {
            let next_cursor = rows.last().map(encode_inbox_cursor);
            return Ok(json!({
                "messages": rows.iter().map(message_row_json).collect::<Vec<_>>(),
                "next_cursor": next_cursor,
            }));
        }
        if Instant::now() >= deadline {
            return Ok(json!({"messages": [], "next_cursor": cursor_arg}));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// `host.msg.thread(thread_id, cursor?, limit≤100)` (requirement 4 /
/// AC2, AC3, AC14).
pub async fn thread(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let thread_id = arg_str(args, "thread_id")?;
    let limit = arg_i64_opt(args, "limit").unwrap_or(50).clamp(1, 100);
    let after_seq = match arg_str_opt(args, "cursor") {
        Some(c) => Some(
            c.parse::<i64>()
                .map_err(|_| AppError::InvalidArgs("cursor: malformed".to_string()))?,
        ),
        None => None,
    };

    let rows = state
        .db
        .msg_thread(tenant.id, thread_id, after_seq, limit)
        .await?
        .ok_or_else(AppError::thread_not_found)?;
    let next_cursor = if rows.len() as i64 == limit {
        rows.last().map(|r| r.seq.to_string())
    } else {
        None
    };
    Ok(json!({
        "messages": rows.iter().map(message_row_json).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    }))
}

/// `host.msg.ack(message_ids)` (requirement 5).
pub async fn ack(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let ids: Vec<String> = match args.get("message_ids") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str().map(str::to_string).ok_or_else(|| {
                    AppError::InvalidArgs("message_ids: every entry must be a string".to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(AppError::InvalidArgs(
                "message_ids: must be an array of message ids".to_string(),
            ));
        }
        None => return Err(AppError::InvalidArgs("missing required argument 'message_ids'".to_string())),
    };
    let acked = state.db.msg_ack(tenant.id, ids).await?;
    Ok(json!({"acked": acked}))
}

/// `host.msg.block(address)` (requirement 9 / AC5).
pub async fn block(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let address = arg_str(args, "address")?;
    let target_id = state
        .db
        .resolve_agent_address(address.clone())
        .await?
        .ok_or_else(AppError::agent_not_found)?;
    state.db.msg_block(tenant.id, target_id).await?;
    Ok(json!({"address": address, "blocked": true}))
}

/// `host.msg.unblock(address)` (requirement 9).
pub async fn unblock(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let address = arg_str(args, "address")?;
    let target_id = state
        .db
        .resolve_agent_address(address.clone())
        .await?
        .ok_or_else(AppError::agent_not_found)?;
    let unblocked = state.db.msg_unblock(tenant.id, target_id).await?;
    Ok(json!({"address": address, "unblocked": unblocked}))
}
