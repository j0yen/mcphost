//! PRD-mcphost-webhook-inbox: the fourth trigger kind (`kind = "webhook"`),
//! its `POST /hook/{hook_id}` HTTP entrypoint, and the
//! `host.trigger.set/get/list/test/replay` support a webhook trigger needs
//! beyond the generic CRUD `triggers.rs` already owns.
//!
//! Unlike `hooks.rs`'s `kind = "event"` (a tenant-registered secret,
//! signature-verified but never stored), a webhook trigger:
//! - generates and returns its own secret once (`host.trigger.set`'s
//!   response), encrypted at rest via [`crate::state::AppState::secrets`]
//!   the same way a tenant's own `host.secret_set` values are, never a
//!   `secrets.rs`-registered name;
//! - is addressed by an opaque `hook_id` (migration 0030), not
//!   `(namespace, tool)` -- requirement 2's "id is not the tenant id";
//! - stores every accepted delivery as a row in the tenant's own
//!   `inbox_<name>` state table (`tenant_state.rs`) before firing, even
//!   while paused (requirement 4) -- `hooks.rs`'s event kind stores
//!   nothing and 404s outright while paused;
//! - shares the `schedules_max` quota (`Db::count_schedule_triggers_for_tenant`),
//!   not `event_triggers_max`.
//!
//! `config_json`'s shape (distinct from a schedule's, an event's, and a
//! message trigger's own, see `triggers.rs`'s `build_config`/
//! `hooks.rs`'s `set_event_trigger`):
//! ```json
//! {"name": "pay", "verify": "hmac" | "none" | "stripe",
//!  "secret_enc": "<hex ciphertext>", "secret_nonce": "<hex nonce>"}
//! ```

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::db::{Tenant, TriggerRow};
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};
use crate::triggers::trigger_invalid;

/// requirement 2's own `Stripe-Signature`-style staleness tolerance for
/// `verify="stripe"` (AC12) -- same 300s default `hooks.rs`'s own
/// Stripe-style verify uses, so a webhook trigger's `stripe` scheme reads
/// exactly like `billing.rs`'s real Stripe webhook already does.
const STRIPE_TOLERANCE_S: i64 = 300;

/// requirement 3: the fixed header allowlist a stored delivery's headers
/// are redacted to (`hooks.rs`'s `allowed_headers` does the same for the
/// `event` kind, but that one is config-shaped -- a webhook trigger's own
/// signature header is always one of these three fixed names, so a plain
/// constant is enough here).
const ALLOWED_HEADERS: &[&str] = &[
    "content-type",
    "user-agent",
    "x-request-id",
    "x-mcphost-signature",
    "x-mcphost-delivery-id",
    "stripe-signature",
];

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn webhook_not_found() -> AppError {
    AppError::Structured {
        code: "webhook_not_found",
        message: "no such webhook".to_string(),
        data: json!({}),
    }
}

fn webhook_signature_invalid() -> AppError {
    AppError::Structured {
        code: "webhook_signature_invalid",
        message: "signature did not match".to_string(),
        data: json!({}),
    }
}

fn webhook_body_too_large(limit_bytes: i64, actual_bytes: usize) -> AppError {
    AppError::Structured {
        code: "webhook_body_too_large",
        message: format!("body is {actual_bytes} bytes, over the plan's {limit_bytes} byte limit"),
        data: json!({"limit_bytes": limit_bytes, "actual_bytes": actual_bytes}),
    }
}

fn webhook_over_quota() -> AppError {
    AppError::Structured {
        code: "webhook_over_quota",
        message: "tenant is over its calls_per_day quota".to_string(),
        data: json!({}),
    }
}

// ---- config -----------------------------------------------------------

struct WebhookConfig {
    name: String,
    verify: String,
    secret_enc: Vec<u8>,
    secret_nonce: Vec<u8>,
}

fn parse_webhook_config(config_json: &str) -> Option<WebhookConfig> {
    let config: Value = serde_json::from_str(config_json).ok()?;
    let name = config.get("name")?.as_str()?.to_string();
    let verify = config.get("verify").and_then(Value::as_str).unwrap_or("hmac").to_string();
    let secret_enc = from_hex(config.get("secret_enc")?.as_str()?)?;
    let secret_nonce = from_hex(config.get("secret_nonce")?.as_str()?)?;
    Some(WebhookConfig {
        name,
        verify,
        secret_enc,
        secret_nonce,
    })
}

fn build_webhook_config(name: &str, verify: &str, secret_enc: &[u8], secret_nonce: &[u8]) -> Value {
    json!({
        "name": name,
        "verify": verify,
        "secret_enc": to_hex(secret_enc),
        "secret_nonce": to_hex(secret_nonce),
    })
}

fn inbox_table_name(name: &str) -> String {
    format!("inbox_{name}")
}

/// `https://<public_url>/hook/<hook_id>` (technical considerations: "the
/// opaque id maps to tenant + trigger in one indexed lookup").
pub fn webhook_url(state: &AppState, hook_id: &str) -> String {
    format!("{}/hook/{}", state.public_url.trim_end_matches('/'), hook_id)
}

// ---- host.trigger.set(kind="webhook") ----------------------------------

/// `host.trigger.set(tool, kind="webhook", name, verify?)` (P0 requirement
/// 1): creates the trigger row, the `inbox_<name>` state table, and
/// returns `{url, secret, ...}` -- the only time the secret is ever in a
/// response (AC1: "`host.trigger.get` afterwards has the url and no
/// secret").
pub async fn set_webhook_trigger(
    state: &AppState,
    tenant: &Tenant,
    tool: &str,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(trigger_invalid(
            "name",
            "name must be a non-empty string of letters, digits and underscores (it becomes the \
             inbox_<name> state table name)"
                .to_string(),
        ));
    }
    let verify = args.get("verify").and_then(Value::as_str).unwrap_or("hmac").to_string();
    if !matches!(verify.as_str(), "hmac" | "none" | "stripe") {
        return Err(trigger_invalid(
            "verify",
            format!("verify: '{verify}' must be one of hmac, none, stripe"),
        ));
    }

    // requirement 5 / AC8: webhooks share the schedules_max quota.
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let existing = state.db.count_schedule_triggers_for_tenant(tenant.id).await?;
    if existing >= plan.schedules_max {
        return Err(AppError::Structured {
            code: "trigger_quota_exceeded",
            message: format!(
                "tenant already holds {existing} schedules/webhooks, the plan maximum of {}",
                plan.schedules_max
            ),
            data: json!({"schedules_max": plan.schedules_max}),
        });
    }

    let secret_plain = format!("whsec_{}", crate::auth::generate_key());
    let (secret_enc, secret_nonce) = state.secrets.encrypt(&secret_plain)?;
    let hook_id = new_ulid();

    let config = build_webhook_config(&name, &verify, &secret_enc, &secret_nonce);
    let config_json = serde_json::to_string(&config)
        .map_err(|e| AppError::Internal(format!("trigger config serialize: {e}")))?;
    let hash = crate::triggers::config_hash(&config_json);
    let id = new_ulid();
    state
        .db
        .insert_trigger(
            id.clone(),
            tenant.id,
            tool.to_string(),
            "webhook".to_string(),
            config_json,
            hash,
            None,
            Some(hook_id),
        )
        .await
        .map_err(|_| {
            trigger_invalid(
                "name",
                "this tool already has an identical webhook trigger".to_string(),
            )
        })?;

    // requirement 3: the inbox table, auto-created -- `received_at`/
    // `headers`/`body` per delivery; `delivery_id` is `text` so an absent
    // header simply omits the field rather than needing a sentinel.
    let schema_json = serde_json::to_string(&json!({
        "received_at": "integer",
        "headers": "json",
        "body": "json",
        "delivery_id": "text",
    }))
    .map_err(|e| AppError::Internal(format!("inbox schema serialize: {e}")))?;
    state
        .db
        .state_table_create(tenant.id, inbox_table_name(&name), schema_json, None)
        .await?;

    let row = state
        .db
        .get_trigger(tenant.id, id.clone())
        .await?
        .ok_or_else(|| AppError::Internal("trigger vanished immediately after insert".to_string()))?;
    let mut out = trigger_to_json_webhook(state, tenant, &row).await;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("secret".to_string(), json!(secret_plain));
    }
    Ok(out)
}

/// The webhook-kind shape of `host.trigger.list`/`get`'s per-trigger JSON
/// -- `url`/`name`/`verify`, never the secret (AC1).
pub(crate) async fn trigger_to_json_webhook(state: &AppState, tenant: &Tenant, row: &TriggerRow) -> Value {
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
    let config = parse_webhook_config(&row.config_json);
    let hook_id = row.hook_id.clone().unwrap_or_default();
    json!({
        "id": row.id,
        "tool": row.tool_name,
        "kind": row.kind,
        "enabled": row.enabled,
        "created_unix": row.created_unix,
        "last_run_id": row.last_run_id,
        "last_fired_unix": row.last_fired_unix,
        "last_status": last_status,
        "url": webhook_url(state, &hook_id),
        "name": config.as_ref().map(|c| c.name.clone()),
        "verify": config.as_ref().map(|c| c.verify.clone()),
    })
}

// ---- delivery accept path (shared by the real route and host.trigger.test) ---

struct AcceptOutcome {
    row_id: i64,
    run_id: Option<String>,
}

fn allowed_headers_json(headers: &HeaderMap) -> Value {
    let mut obj = serde_json::Map::new();
    for (name, value) in headers.iter() {
        let lname = name.as_str().to_ascii_lowercase();
        if ALLOWED_HEADERS.contains(&lname.as_str()) {
            obj.insert(lname, json!(value.to_str().unwrap_or("")));
        }
    }
    Value::Object(obj)
}

fn parse_body_value(body: &[u8]) -> Value {
    serde_json::from_slice::<Value>(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).to_string()))
}

fn verify_delivery(scheme: &str, secret: &str, body: &[u8], headers: &HeaderMap) -> Result<(), AppError> {
    match scheme {
        "none" => Ok(()),
        "hmac" => {
            let got = headers
                .get("X-Mcphost-Signature")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let sig = got.strip_prefix("sha256=").unwrap_or(got);
            let expected = to_hex(&crate::billing::hmac_sha256(secret.as_bytes(), body));
            if !sig.is_empty() && constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(webhook_signature_invalid())
            }
        }
        "stripe" => {
            let header_value = headers
                .get("Stripe-Signature")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            crate::billing::verify_stripe_signature(body, header_value, secret, STRIPE_TOLERANCE_S, now_unix())
                .map_err(|_| webhook_signature_invalid())
        }
        _ => Err(webhook_signature_invalid()),
    }
}

/// requirement 2/3/5/6's whole accept path: quota, verify, dedupe, insert,
/// fire. Shared by the real HTTP route ([`hook_receive`]) and
/// `host.trigger.test`'s webhook branch ([`test_webhook_trigger`]) --
/// requirement 8 / AC11's own "one synthetic signed delivery is stored and
/// fired" is exactly this function run against a self-signed synthetic
/// body, not a second implementation. Six parameters: one accept path, two
/// call sites, same "no struct just to dodge the lint" call
/// `hooks::enqueue_with_dedupe`'s own doc comment already explains this
/// crate makes when a struct would only bundle this call's own inputs.
#[allow(clippy::too_many_arguments)]
async fn accept_delivery(
    state: &AppState,
    tenant: &Tenant,
    trigger: &TriggerRow,
    config: &WebhookConfig,
    body: &[u8],
    headers: &HeaderMap,
) -> Result<AcceptOutcome, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;

    if body.len() > plan.event_body_bytes_max as usize {
        return Err(webhook_body_too_large(plan.event_body_bytes_max, body.len()));
    }

    // requirement 2's 429: `calls_per_day`, checked (not atomically -- same
    // best-effort posture `handler.rs`'s own `check_calls_quota` already
    // takes) before the secret is even decrypted, so an over-quota tenant
    // never pays for a signature check either.
    let midnight = crate::state::utc_midnight_unix(now_unix());
    let used = state.db.count_calls_since(tenant.id, midnight, true).await?;
    if used >= plan.calls_per_day {
        return Err(webhook_over_quota());
    }

    let secret_plain = state.secrets.decrypt(&config.secret_enc, &config.secret_nonce)?;
    verify_delivery(&config.verify, &secret_plain, body, headers)?;

    let delivery_id = headers
        .get("X-Mcphost-Delivery-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(key) = &delivery_id {
        let fresh = state.db.claim_webhook_dedupe(trigger.id.clone(), key.clone()).await?;
        if !fresh {
            // requirement 6 / AC7: seen within 24h -- 200, nothing new
            // stored, nothing fired. There is no row to name here (the
            // original row is still the tenant's only row for this
            // delivery id), so the caller answers with no row/run id.
            return Ok(AcceptOutcome { row_id: -1, run_id: None });
        }
    }

    // `delivery_id` is a declared `text` column (schema_violation on
    // anything else, including `null`) -- omitted entirely rather than
    // included as `null` when the sender sent no `X-Mcphost-Delivery-Id`,
    // since `tenant_state::validate_row` only checks fields actually
    // present in the row map.
    let mut row_obj = serde_json::Map::new();
    row_obj.insert("received_at".to_string(), json!(now_unix()));
    row_obj.insert("headers".to_string(), allowed_headers_json(headers));
    row_obj.insert("body".to_string(), parse_body_value(body));
    if let Some(key) = &delivery_id {
        row_obj.insert("delivery_id".to_string(), json!(key));
    }
    let row = Value::Object(row_obj);
    let insert_result = crate::tenant_state::state_insert(
        state,
        tenant,
        &json!({"table": inbox_table_name(&config.name), "rows": row.clone()}),
        None,
    )
    .await?;
    let row_id = insert_result["ids"][0]
        .as_i64()
        .ok_or_else(|| AppError::Internal("state_insert did not return a row id".to_string()))?;

    let run_id = if trigger.enabled {
        let args_json = serde_json::to_string(&row)
            .map_err(|e| AppError::Internal(format!("webhook row serialize: {e}")))?;
        Some(
            crate::hooks::enqueue_with_dedupe(
                state,
                tenant,
                trigger,
                crate::hooks::EnqueueSpec {
                    trigger_kind: "webhook",
                    dedupe_key: None,
                    message_id: None,
                    args_json,
                },
            )
            .await?,
        )
    } else {
        None
    };

    let _ = state
        .db
        .record_call(
            tenant.id,
            trigger.tool_name.clone(),
            0,
            true,
            None,
            None,
            None,
            "accepted",
            tenant.origin.clone(),
            tenant.origin_detail.clone(),
        )
        .await;

    Ok(AcceptOutcome { row_id, run_id })
}

// ---- POST /hook/{id} ----------------------------------------------------

fn status_for_webhook_error(code: &str) -> StatusCode {
    match code {
        "webhook_not_found" => StatusCode::NOT_FOUND,
        "webhook_signature_invalid" => StatusCode::UNAUTHORIZED,
        "webhook_body_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "webhook_over_quota" => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// AC4: an unknown id (or any other rejection) answers with an empty body
/// -- unlike `hooks.rs`'s event route, which returns a JSON error object,
/// a webhook's opaque id is meant to leak nothing about why it failed.
fn webhook_error_response(err: AppError) -> Response {
    let status = status_for_webhook_error(err.code());
    (status, ()).into_response()
}

/// `POST /hook/{id}` -- `http.rs` mounts this route.
pub async fn hook_receive(State(state): State<Arc<AppState>>, Path(hook_id): Path<String>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let lookup = match state.db.find_trigger_by_hook_id(hook_id).await {
        Ok(Some(trigger)) if trigger.kind == "webhook" => trigger,
        Ok(_) => return webhook_error_response(webhook_not_found()),
        Err(e) => return webhook_error_response(e),
    };
    let tenant = match state.db.find_tenant_by_id(lookup.tenant_id).await {
        Ok(Some(t)) if !t.disabled => t,
        Ok(_) => return webhook_error_response(webhook_not_found()),
        Err(e) => return webhook_error_response(e),
    };
    let Some(config) = parse_webhook_config(&lookup.config_json) else {
        return webhook_error_response(AppError::Internal("webhook trigger config is corrupt".to_string()));
    };

    match accept_delivery(&state, &tenant, &lookup, &config, &body, &headers).await {
        Ok(outcome) => (
            StatusCode::OK,
            Json(json!({"row_id": outcome.row_id, "run_id": outcome.run_id})),
        )
            .into_response(),
        Err(err) => webhook_error_response(err),
    }
}

// ---- host.trigger.test / host.trigger.replay ----------------------------

/// `host.trigger.test(id)` on a webhook trigger (P1 requirement 8 / AC11):
/// builds a synthetic body, signs it exactly the way a real sender would
/// (so the round trip through [`accept_delivery`] proves the secret really
/// works), and runs it through the identical accept path a real POST
/// would.
pub async fn test_webhook_trigger(state: &AppState, tenant: &Tenant, row: &TriggerRow, args: &Value) -> Result<Value, AppError> {
    let config = parse_webhook_config(&row.config_json)
        .ok_or_else(|| AppError::Internal("webhook trigger config is corrupt".to_string()))?;
    let secret_plain = state.secrets.decrypt(&config.secret_enc, &config.secret_nonce)?;

    let body_value = args.get("body").cloned().unwrap_or_else(|| json!({"test": true}));
    let body_bytes = serde_json::to_vec(&body_value).unwrap_or_default();

    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    headers.insert(
        "x-mcphost-delivery-id",
        format!("test-{}", new_ulid()).parse().unwrap(),
    );
    match config.verify.as_str() {
        "hmac" => {
            let sig = to_hex(&crate::billing::hmac_sha256(secret_plain.as_bytes(), &body_bytes));
            headers.insert("x-mcphost-signature", format!("sha256={sig}").parse().unwrap());
        }
        "stripe" => {
            let t = now_unix();
            let signed_payload = [t.to_string().as_bytes(), b".", body_bytes.as_slice()].concat();
            let sig = to_hex(&crate::billing::hmac_sha256(secret_plain.as_bytes(), &signed_payload));
            headers.insert("stripe-signature", format!("t={t},v1={sig}").parse().unwrap());
        }
        _ => {}
    }

    let outcome = accept_delivery(state, tenant, row, &config, &body_bytes, &headers).await?;
    Ok(json!({"row_id": outcome.row_id, "run_id": outcome.run_id, "test": true}))
}

/// `host.trigger.replay(id, row_id)` on a webhook trigger (AC6): re-fires
/// the tool with a previously stored row's own data verbatim -- no
/// re-verification, the same "the original delivery already passed it"
/// rationale `hooks::replay`'s own doc comment gives for the run_id-based
/// replay of an event/message-triggered run. A webhook trigger needs its
/// own row-id-addressed replay (rather than reusing `hooks::replay`'s
/// run_id) because a paused delivery is stored with no run at all
/// (requirement 4) -- there is no run to name.
pub async fn replay_webhook_row(state: &AppState, tenant: &Tenant, row: &TriggerRow, row_id: i64) -> Result<Value, AppError> {
    let config = parse_webhook_config(&row.config_json)
        .ok_or_else(|| AppError::Internal("webhook trigger config is corrupt".to_string()))?;
    let rows = state.db.state_rows_all(tenant.id, inbox_table_name(&config.name)).await?;
    let (_, row_json) = rows
        .into_iter()
        .find(|(id, _)| *id == row_id)
        .ok_or_else(|| {
            AppError::Structured {
                code: "webhook_row_not_found",
                message: format!("no inbox row '{row_id}' for this webhook trigger"),
                data: json!({"row_id": row_id}),
            }
        })?;

    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let new_run_id = new_ulid();
    state
        .db
        .insert_queued_run(
            new_run_id.clone(),
            tenant.id,
            row.tool_name.clone(),
            "webhook".to_string(),
            Some(row.id.clone()),
            None,
            plan.job_max_s,
            row_json,
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .await?;
    Ok(json!({"run_id": new_run_id, "status": "queued", "replay_of_row": row_id}))
}

/// `host.trigger.replay`'s own args parsing reads `row_id` before it knows
/// whether the trigger is a webhook at all -- shared here so `hooks.rs`
/// doesn't need its own copy of "an integer, or absent".
pub(crate) fn arg_i64(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}
