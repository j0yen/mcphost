//! PRD-mcphost-webhook-inbox-one-call: `host.webhook_inbox.create` -- a
//! webhook inbox is one host call.
//!
//! One call publishes the reader tool `<name>` (python, no network field),
//! sets one `kind = "webhook"` trigger bound to it and returns the public
//! `url`, the one-time `secret` and the `host.msg.wait` arguments that read
//! the next event. The trigger machinery is the existing one
//! ([`crate::webhooks`]); the only addition on the delivery path is that a
//! trigger created here carries `inbox: true` in its config, which makes an
//! accepted delivery also post one `@host` message to the tenant inbox
//! ([`notify_event`]).

use serde_json::{Value, json};

use crate::db::{Tenant, TriggerRow};
use crate::errors::AppError;
use crate::state::AppState;

/// The reader tool's source; `__TABLE__` is the trigger's `inbox_<name>`
/// state table. A constant, so the sandbox compile test covers exactly what
/// every inbox runs.
pub const READER_SOURCE: &str = r#"import mcphost


def main(args):
    res = mcphost.state.query(table="__TABLE__", order_by="received_at desc", limit=50)
    events = []
    for row in res["rows"]:
        body = row.get("body")
        event_id = None
        if isinstance(body, dict) and isinstance(body.get("id"), (str, int)):
            event_id = str(body["id"])
        if event_id is None:
            event_id = row.get("delivery_id")
        events.append({
            "id": event_id,
            "received_at": row.get("received_at"),
            "delivery_id": row.get("delivery_id"),
            "body": body,
        })
    return {"rows": events, "count": len(events)}
"#;

/// The verify schemes `host.webhook_inbox.create` offers (a subset of what
/// `host.trigger.set` accepts: `none` is never offered for an inbox).
pub const VERIFY_SCHEMES: &[&str] = &["hmac", "github", "stripe"];

/// Message `data.kind` of the per-event notice.
pub const NOTICE_KIND: &str = "webhook_inbox";

/// How many stored events the reader returns.
pub const READER_LIMIT: usize = 50;

pub fn reader_source(name: &str) -> String {
    READER_SOURCE.replace("__TABLE__", &crate::webhooks::inbox_table_name(name))
}

fn thread_id(tenant_id: i64, name: &str) -> String {
    format!("webhook-inbox-{tenant_id}-{name}")
}

/// `host.msg.wait`'s arguments for this inbox: the unread messages, held up
/// to the call's 25 s ceiling.
pub fn wait_args() -> Value {
    json!({"timeout_s": 25, "unread_only": true})
}

fn plan_limit(field: &str, limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: crate::handler::PLAN_LIMIT,
        message: format!("this plan allows {limit} {field}; this tenant already holds {used}"),
        data: json!({"limit": limit, "used": used, "field": field}),
    }
}

fn response(state: &AppState, tenant: &Tenant, name: &str, row: &TriggerRow, verify: &str) -> Value {
    let hook_id = row.hook_id.clone().unwrap_or_default();
    json!({
        "url": crate::webhooks::webhook_url(state, &hook_id),
        "tool": name,
        "trigger_id": row.id,
        "verify": verify,
        "wait": {"tool": "host.msg.wait", "args": wait_args()},
        "next": [
            {
                "tool": "host.msg.wait",
                "args_example": wait_args(),
                "note": "returns one message per accepted delivery; data.event_id names the event",
            },
            {
                "tool": "host.tool_call",
                "args_example": {"name": name},
                "note": format!("the last {READER_LIMIT} stored events of {}", tenant.namespace),
            },
        ],
    })
}

fn signing_note(verify: &str) -> &'static str {
    match verify {
        "github" => "sign the raw body with HMAC-SHA256 and send X-Hub-Signature-256: sha256=<hex>",
        "stripe" => "send Stripe-Signature: t=<unix>,v1=<hex hmac-sha256 of '<t>.<body>'>",
        _ => "sign the raw body with HMAC-SHA256 and send X-Mcphost-Signature: sha256=<hex>",
    }
}

/// `host.webhook_inbox.create(name, verify?, rotate?)`.
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    if let Some(known) = crate::handler::known_control_plane_args("host.webhook_inbox.create", &state.kinds) {
        crate::errors::check_unknown_argument(args, "host.webhook_inbox.create", &known)?;
    }
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
        .to_string();
    crate::state::validate_tool_name(&name)?;
    let verify = args.get("verify").and_then(Value::as_str).unwrap_or("hmac").to_string();
    if !VERIFY_SCHEMES.contains(&verify.as_str()) {
        return Err(AppError::InvalidArgs(format!(
            "verify: '{verify}' must be one of {}",
            VERIFY_SCHEMES.join(", ")
        )));
    }
    let rotate = args.get("rotate").and_then(Value::as_bool).unwrap_or(false);

    // Idempotent by name: the same URL comes back, the secret never does
    // unless the caller rotates it.
    if let Some(existing) = state.db.get_trigger_by_name(tenant.id, name.clone()).await? {
        let config = crate::webhooks::parse_webhook_config(&existing.config_json);
        if existing.kind != "webhook" || !config.as_ref().is_some_and(|c| c.inbox) {
            return Err(AppError::Structured {
                code: crate::triggers::TRIGGER_KIND_MISMATCH,
                message: format!("'{name}' is already a trigger that is not a webhook inbox"),
                data: json!({"name": name, "existing_kind": existing.kind}),
            });
        }
        let existing_verify = config.map(|c| c.verify).unwrap_or(verify);
        let mut out = response(state, tenant, &name, &existing, &existing_verify);
        if rotate {
            let secret = crate::webhooks::rotate_secret(state, &existing).await?;
            out["secret"] = json!(secret);
        }
        out["created"] = json!(false);
        return Ok(out);
    }

    // Plan ceilings are checked before anything exists: a refused create
    // leaves no tool, trigger or event table behind.
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let event_used = state.db.count_event_triggers_for_tenant(tenant.id).await?
        + state.db.count_webhook_triggers_for_tenant(tenant.id).await?;
    if event_used >= plan.event_triggers_max {
        return Err(plan_limit("event_triggers_max", plan.event_triggers_max, event_used));
    }
    let schedule_used = state.db.count_schedule_triggers_for_tenant(tenant.id).await?;
    if schedule_used >= plan.schedules_max {
        return Err(plan_limit("schedules_max", plan.schedules_max, schedule_used));
    }

    crate::control::tool_publish(
        state,
        tenant,
        &json!({"name": name, "kind": "python", "spec": {"source": reader_source(&name)}}),
    )
    .await?;
    let set = crate::webhooks::set_webhook_trigger(
        state,
        tenant,
        &name,
        &name,
        &json!({"verify": verify, "inbox": true}),
        None,
    )
    .await;
    let set = match set {
        Ok(v) => v,
        Err(e) => {
            let _ = state.db.remove_tool(tenant.id, name.clone()).await;
            return Err(e);
        }
    };
    let trigger_id = set.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let row = state
        .db
        .get_trigger(tenant.id, trigger_id)
        .await?
        .ok_or_else(|| AppError::Internal("webhook inbox trigger vanished after create".to_string()))?;
    let mut out = response(state, tenant, &name, &row, &verify);
    out["secret"] = set.get("secret").cloned().unwrap_or(Value::Null);
    out["signing"] = json!(signing_note(&verify));
    out["created"] = json!(true);
    Ok(out)
}

/// The id an event is known by: the body's own `id`, else the sender's
/// delivery id, else the stored row's id.
pub fn event_id_of(body: &Value, delivery_id: Option<&str>, row_id: i64) -> String {
    match body.get("id") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => delivery_id.map(str::to_string).unwrap_or_else(|| row_id.to_string()),
    }
}

/// R2: one `@host` message per accepted delivery, so `host.msg.wait` returns
/// it. Best-effort -- the stored row and the fired run are the delivery; a
/// failed notice is logged, never a failed delivery.
#[allow(clippy::too_many_arguments)]
pub async fn notify_event(
    state: &AppState,
    tenant: &Tenant,
    name: &str,
    tool: &str,
    row_id: i64,
    body: &Value,
    delivery_id: Option<&str>,
) {
    let event_id = event_id_of(body, delivery_id, row_id);
    let data = json!({"kind": NOTICE_KIND, "event_id": event_id, "tool": tool, "row_id": row_id});
    if let Err(e) = state
        .db
        .insert_host_notice(
            tenant.id,
            thread_id(tenant.id, name),
            "@host".to_string(),
            format!("webhook {name}: event {event_id}"),
            data.to_string(),
        )
        .await
    {
        tracing::warn!(error = %e, tenant = %tenant.namespace, "webhook inbox notice failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_prefers_body_id_then_delivery_then_row() {
        assert_eq!(event_id_of(&json!({"id": "evt-1"}), Some("d"), 3), "evt-1");
        assert_eq!(event_id_of(&json!({"id": 7}), Some("d"), 3), "7");
        assert_eq!(event_id_of(&json!({"x": 1}), Some("d"), 3), "d");
        assert_eq!(event_id_of(&json!("raw"), None, 3), "3");
    }

    #[test]
    fn reader_source_names_the_inbox_table() {
        assert!(reader_source("inbox").contains("table=\"inbox_inbox\""));
        assert!(!reader_source("inbox").contains("__TABLE__"));
    }
}
