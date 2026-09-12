//! PRD-mcphost-inbound-events: the second trigger kind (`kind = "event"`),
//! its `POST /hooks/{namespace}/{tool}` HTTP entrypoint, the four
//! signature-verification schemes, and the tenant-facing
//! `host.trigger.test`/`host.trigger.replay` tools.
//!
//! `triggers.rs` owns the generic `host.trigger.set/list/get/pause/resume/
//! remove` CRUD (an event trigger is just another `triggers` row, `kind =
//! "event"`) and dispatches `kind = "event"` into [`set_event_trigger`]
//! here; this module owns everything specific to firing one from the
//! outside: the axum handler `http.rs` mounts at `POST
//! /hooks/{namespace}/{tool}`, the per-trigger rate limiter, the host-wide
//! received/rejected counters `/healthz` reports, and dedupe.
//!
//! An event trigger's `config_json` shape (distinct from a schedule's
//! `{schedule, args, tz}`, see `triggers.rs`'s `build_config`):
//! ```json
//! {"verify": {"scheme": "hmac-sha256", "header": "...", "secret": "...", ...},
//!  "dedupe_header": "X-GitHub-Delivery",
//!  "args": {}}
//! ```
//! `verify.secret` is a *secret name* (resolved through `secrets.rs` at
//! verify time), never a plaintext value -- safe to return from
//! `host.trigger.list`/`get`/admin.triggers.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::db::{Tenant, TriggerRow};
use crate::errors::AppError;
use crate::state::{now_unix, new_ulid, AppState};

/// requirement 2: Stripe-style `t=,v1=` staleness window, used when a
/// `hmac-sha256` verify config names a `timestamp_header` without its own
/// `tolerance_s`.
const DEFAULT_TOLERANCE_S: i64 = 300;

/// `/healthz`'s `events_received_1h`/`events_rejected_1h` (requirement 6).
const COUNTER_WINDOW: Duration = Duration::from_secs(3600);

/// A per-trigger sliding-window rate limit's own window (requirement 4:
/// "events_per_minute ... per trigger").
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn trigger_invalid(field: &'static str, message: String) -> AppError {
    AppError::Structured {
        code: "trigger_invalid",
        message,
        data: json!({"field": field}),
    }
}

fn hook_not_found() -> AppError {
    AppError::Structured {
        code: "hook_not_found",
        message: "no enabled event trigger for this tool".to_string(),
        data: json!({}),
    }
}

fn signature_invalid(header: &str) -> AppError {
    AppError::Structured {
        code: "signature_invalid",
        message: format!("signature in '{header}' did not match"),
        data: json!({"header": header}),
    }
}

/// AC3: the Stripe-style timestamp-staleness rejection names `reason:
/// "timestamp"` specifically, distinct from an ordinary mismatch.
fn signature_invalid_reason(reason: &'static str) -> AppError {
    AppError::Structured {
        code: "signature_invalid",
        message: format!("signature check failed: {reason}"),
        data: json!({"reason": reason}),
    }
}

fn run_not_found(run_id: &str) -> AppError {
    AppError::Structured {
        code: "run_not_found",
        message: format!("no run '{run_id}' for this tenant"),
        data: json!({"run_id": run_id}),
    }
}

// ---- host-wide `/healthz` counters (requirement 6) ------------------------

/// `events_received_1h`/`events_rejected_1h`: process-local sliding-window
/// counts, same "in-memory, not since-boot-persisted" posture as
/// [`crate::triggers::SchedulerStatus`] -- a restart resets both to zero,
/// which is fine for a rolling-hour gauge.
#[derive(Clone, Default)]
pub struct EventCounters {
    received: Arc<Mutex<VecDeque<Instant>>>,
    rejected: Arc<Mutex<VecDeque<Instant>>>,
}

impl EventCounters {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(deque: &Mutex<VecDeque<Instant>>) {
        let now = Instant::now();
        let mut guard = deque.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(&oldest) = guard.front() {
            if now.duration_since(oldest) >= COUNTER_WINDOW {
                guard.pop_front();
            } else {
                break;
            }
        }
        guard.push_back(now);
    }

    fn count(deque: &Mutex<VecDeque<Instant>>) -> i64 {
        let now = Instant::now();
        let mut guard = deque.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(&oldest) = guard.front() {
            if now.duration_since(oldest) >= COUNTER_WINDOW {
                guard.pop_front();
            } else {
                break;
            }
        }
        guard.len() as i64
    }

    /// Every request that reaches `POST /hooks/...`, whatever the outcome.
    pub fn record_received(&self) {
        Self::record(&self.received);
    }

    /// Every non-`202` outcome (404/401/413/429).
    pub fn record_rejected(&self) {
        Self::record(&self.rejected);
    }

    pub fn received_1h(&self) -> i64 {
        Self::count(&self.received)
    }

    pub fn rejected_1h(&self) -> i64 {
        Self::count(&self.rejected)
    }
}

/// requirement 4's `events_per_minute` ceiling, enforced per trigger id --
/// same sliding-window shape as [`crate::state::ToolRunLimiter`], keyed by
/// trigger id (a `String`, unlike that limiter's `i64` tenant id) since
/// this crate's trigger ids are ULIDs, not row ids.
#[derive(Clone, Default)]
pub struct EventRateLimiter {
    windows: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

impl EventRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` if `trigger_id` has room for one more event in the current
    /// 60s window under `limit_per_minute` (and records this one if so).
    pub fn allow(&self, trigger_id: &str, limit_per_minute: i64) -> bool {
        let now = Instant::now();
        let mut guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.entry(trigger_id.to_string()).or_default();
        while let Some(&oldest) = window.front() {
            if now.duration_since(oldest) >= RATE_LIMIT_WINDOW {
                window.pop_front();
            } else {
                break;
            }
        }
        if window.len() as i64 >= limit_per_minute {
            return false;
        }
        window.push_back(now);
        true
    }
}

// ---- signature verification (requirement 2) -------------------------------

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
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

/// GitHub's legacy `X-Hub-Signature` scheme: HMAC-SHA1. `sha1` (RustCrypto,
/// same family as `sha2`) supplies the compression function; the HMAC
/// construction itself is hand-rolled the same way `billing::hmac_sha256`
/// is, since this crate has no `hmac` crate dependency either way.
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    use sha1::{Digest, Sha1};
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha1::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha1::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn hmac_hex(scheme: &str, secret: &[u8], message: &[u8]) -> String {
    if scheme == "hmac-sha1" {
        to_hex(&hmac_sha1(secret, message))
    } else {
        to_hex(&crate::billing::hmac_sha256(secret, message))
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// Stripe-style `t=<unix>,v1=<hex>[,v1=<hex>...]` embedded in one header's
/// own value (requirement 2's "optional timestamp header and tolerance for
/// Stripe-style t=,v1= values"; AC3). Mirrors
/// `billing::verify_stripe_signature`'s parse, but returns this module's
/// own `signature_invalid`/`reason` shape rather than `invalid_webhook_signature`.
fn verify_stripe_style(
    scheme: &str,
    body: &[u8],
    header_value: &str,
    secret: &str,
    tolerance_s: i64,
) -> Result<(), AppError> {
    let mut timestamp: Option<i64> = None;
    let mut signatures: Vec<&str> = Vec::new();
    for part in header_value.split(',') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        match k {
            "t" => timestamp = v.parse::<i64>().ok(),
            "v1" => signatures.push(v),
            _ => {}
        }
    }
    let Some(t) = timestamp else {
        return Err(signature_invalid_reason("format"));
    };
    if signatures.is_empty() {
        return Err(signature_invalid_reason("format"));
    }
    if (now_unix() - t).abs() > tolerance_s {
        return Err(signature_invalid_reason("timestamp"));
    }
    let signed_payload = [t.to_string().as_bytes(), b".", body].concat();
    let expected = hmac_hex(scheme, secret.as_bytes(), &signed_payload);
    let matched = signatures
        .iter()
        .any(|sig| constant_time_eq(sig.as_bytes(), expected.as_bytes()));
    if matched {
        Ok(())
    } else {
        Err(signature_invalid_reason("mismatch"))
    }
}

/// Checks one inbound request against a trigger's stored `verify` config
/// (requirement 2). `secret_plain` is `None` only for `scheme = "none"`
/// (every other scheme's config was validated at `host.trigger.set` time to
/// carry a `secret` name, and the caller resolves it before calling this).
fn verify_event(
    verify: &Value,
    secret_plain: Option<&str>,
    body: &[u8],
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let scheme = verify.get("scheme").and_then(Value::as_str).unwrap_or("none");
    match scheme {
        "none" => Ok(()),
        "token" => {
            let header_name = verify.get("header").and_then(Value::as_str).unwrap_or("");
            let expected = secret_plain.unwrap_or("");
            let got = header_str(headers, header_name);
            if !expected.is_empty() && constant_time_eq(got.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(signature_invalid(header_name))
            }
        }
        "hmac-sha256" | "hmac-sha1" => {
            let header_name = verify.get("header").and_then(Value::as_str).unwrap_or("");
            let secret = secret_plain.unwrap_or("");

            if let Some(ts_header) = verify.get("timestamp_header").and_then(Value::as_str) {
                let tolerance = verify
                    .get("tolerance_s")
                    .and_then(Value::as_i64)
                    .unwrap_or(DEFAULT_TOLERANCE_S);
                let header_value = header_str(headers, ts_header);
                return verify_stripe_style(scheme, body, header_value, secret, tolerance);
            }

            let prefix = verify.get("prefix").and_then(Value::as_str).unwrap_or("");
            let raw_header = header_str(headers, header_name);
            let sig = raw_header.strip_prefix(prefix).unwrap_or(raw_header);
            let expected = hmac_hex(scheme, secret.as_bytes(), body);
            if !sig.is_empty() && constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(signature_invalid(header_name))
            }
        }
        _ => Err(signature_invalid("")),
    }
}

// ---- verify-config validation (host.trigger.set kind="event") -------------

/// Validates and canonicalizes `host.trigger.set(kind="event", verify=...)`'s
/// `verify` argument into the shape stored in `config_json` (requirement 2;
/// AC4). Pure -- the caller checks the named secret actually exists
/// separately, since that needs the tenant's own secret store.
fn validate_verify_config(verify: &Value) -> Result<Value, AppError> {
    let scheme = verify.get("scheme").and_then(Value::as_str).unwrap_or("");
    match scheme {
        "hmac-sha256" | "hmac-sha1" | "token" => {
            let header = verify
                .get("header")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    trigger_invalid("verify.header", "verify.header is required for this scheme".to_string())
                })?;
            let secret = verify
                .get("secret")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    trigger_invalid("verify.secret", "verify.secret is required for this scheme".to_string())
                })?;
            let mut out = json!({"scheme": scheme, "header": header, "secret": secret});
            if scheme != "token"
                && let Some(prefix) = verify.get("prefix").and_then(Value::as_str)
            {
                out["prefix"] = json!(prefix);
            }
            // requirement 2: only hmac-* schemes carry the Stripe-style
            // timestamp option.
            if scheme != "token"
                && let Some(ts_header) = verify.get("timestamp_header").and_then(Value::as_str)
            {
                out["timestamp_header"] = json!(ts_header);
                let tolerance = verify
                    .get("tolerance_s")
                    .and_then(Value::as_i64)
                    .unwrap_or(DEFAULT_TOLERANCE_S);
                out["tolerance_s"] = json!(tolerance);
            }
            Ok(out)
        }
        "none" => {
            let allow_unverified = verify
                .get("allow_unverified")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !allow_unverified {
                return Err(trigger_invalid(
                    "verify.allow_unverified",
                    "verify.scheme: \"none\" needs verify.allow_unverified: true -- an inbound \
                     event trigger with no signature check must opt in explicitly"
                        .to_string(),
                ));
            }
            Ok(json!({"scheme": "none", "allow_unverified": true}))
        }
        other => Err(trigger_invalid(
            "verify.scheme",
            format!(
                "verify.scheme: '{other}' must be one of hmac-sha256, hmac-sha1, token, none"
            ),
        )),
    }
}

async fn resolve_secret(
    state: &AppState,
    tenant_id: i64,
    verify: &Value,
) -> Result<Option<String>, AppError> {
    let Some(name) = verify.get("secret").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some((enc, nonce)) = state.db.get_secret(tenant_id, name.to_string()).await? else {
        return Err(AppError::SecretMissing(name.to_string()));
    };
    Ok(Some(state.secrets.decrypt(&enc, &nonce)?))
}

/// `https://<public_url>/hooks/<namespace>/<tool>` (technical
/// considerations).
pub fn event_url(state: &AppState, namespace: &str, tool: &str) -> String {
    format!(
        "{}/hooks/{}/{}",
        state.public_url.trim_end_matches('/'),
        namespace,
        tool
    )
}

/// `host.trigger.set(tool, kind="event", verify, dedupe_header?, args?)`
/// (P0 requirement 3). Called from `triggers::set` once the tool itself is
/// known to exist; enforces "at most one event trigger per tool" (the URL
/// is keyed by `(namespace, tool)` alone, with no trigger id in the path,
/// so a second one would be unreachable) and the plan's `event_triggers_max`
/// (requirement 4).
pub async fn set_event_trigger(
    state: &AppState,
    tenant: &Tenant,
    tool: &str,
    args: &Value,
) -> Result<Value, AppError> {
    let existing_on_tool = state.db.list_triggers(tenant.id, Some(tool.to_string())).await?;
    if existing_on_tool.iter().any(|t| t.kind == "event") {
        return Err(trigger_invalid(
            "tool",
            format!(
                "'{tool}' already has an event trigger; host.trigger.remove it before adding \
                 another (the hook URL is keyed by tool name alone)"
            ),
        ));
    }

    let verify_arg = args
        .get("verify")
        .cloned()
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'verify'".to_string()))?;
    let verify = validate_verify_config(&verify_arg)?;
    if let Some(secret_name) = verify.get("secret").and_then(Value::as_str) {
        state
            .db
            .get_secret(tenant.id, secret_name.to_string())
            .await?
            .ok_or_else(|| AppError::SecretMissing(secret_name.to_string()))?;
    }

    let dedupe_header = args.get("dedupe_header").and_then(Value::as_str).map(str::to_string);
    let call_args = args.get("args").cloned().unwrap_or_else(|| json!({}));

    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let existing = state.db.count_event_triggers_for_tenant(tenant.id).await?;
    if existing >= plan.event_triggers_max {
        return Err(AppError::Structured {
            code: "trigger_quota_exceeded",
            message: format!(
                "tenant already holds {existing} event triggers, the plan maximum of {}",
                plan.event_triggers_max
            ),
            data: json!({"event_triggers_max": plan.event_triggers_max}),
        });
    }

    let mut config = json!({"verify": verify, "args": call_args});
    if let Some(dh) = &dedupe_header
        && let Some(obj) = config.as_object_mut()
    {
        obj.insert("dedupe_header".to_string(), json!(dh));
    }
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
            "event".to_string(),
            config_json,
            hash,
            None,
        )
        .await
        .map_err(|_| {
            trigger_invalid(
                "verify",
                "this tool already has an identical event trigger".to_string(),
            )
        })?;

    let row = state
        .db
        .get_trigger(tenant.id, id.clone())
        .await?
        .ok_or_else(|| AppError::Internal("trigger vanished immediately after insert".to_string()))?;
    Ok(trigger_to_json_event(state, tenant, &row).await)
}

/// The event-kind shape of `host.trigger.list`/`get`'s per-trigger JSON --
/// `url`/`verify`/`unverified`/`dedupe_header` instead of a schedule's
/// `schedule`/`args`/`tz`.
pub(crate) async fn trigger_to_json_event(state: &AppState, tenant: &Tenant, row: &TriggerRow) -> Value {
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
    let config: Value = serde_json::from_str(&row.config_json).unwrap_or_else(|_| json!({}));
    let verify = config.get("verify").cloned().unwrap_or(json!({}));
    let unverified = verify.get("scheme").and_then(Value::as_str) == Some("none");
    json!({
        "id": row.id,
        "tool": row.tool_name,
        "kind": row.kind,
        "enabled": row.enabled,
        "created_unix": row.created_unix,
        "last_run_id": row.last_run_id,
        "last_fired_unix": row.last_fired_unix,
        "last_status": last_status,
        "url": event_url(state, &tenant.namespace, &row.tool_name),
        "verify": verify,
        "unverified": unverified,
        "dedupe_header": config.get("dedupe_header").cloned().unwrap_or(Value::Null),
    })
}

// ---- building the run args (requirement 1 / requirement 5) ----------------

/// requirement 5: the header allowlist a stored/forwarded event's headers
/// are redacted to -- the fixed base set plus whatever the trigger config
/// itself names (its own signature/timestamp header, and `dedupe_header`).
fn allowed_headers(verify: &Value, dedupe_header: Option<&str>) -> Vec<String> {
    let mut allow = vec![
        "content-type".to_string(),
        "user-agent".to_string(),
        "x-request-id".to_string(),
    ];
    if let Some(h) = verify.get("header").and_then(Value::as_str) {
        allow.push(h.to_ascii_lowercase());
    }
    if let Some(h) = verify.get("timestamp_header").and_then(Value::as_str) {
        allow.push(h.to_ascii_lowercase());
    }
    if let Some(h) = dedupe_header {
        allow.push(h.to_ascii_lowercase());
    }
    allow
}

fn redacted_headers_json(headers: &HeaderMap, allow: &[String]) -> Value {
    let mut obj = serde_json::Map::new();
    for (name, value) in headers.iter() {
        let lname = name.as_str().to_ascii_lowercase();
        if allow.contains(&lname) {
            obj.insert(lname, json!(value.to_str().unwrap_or("")));
        }
    }
    Value::Object(obj)
}

/// Most webhook bodies are JSON; a body that doesn't parse is kept as a
/// (lossy) string rather than dropped, so a non-JSON sender's payload is
/// still inspectable via `host.runs.get`/`host.trigger.replay` instead of
/// silently becoming `null`.
fn parse_body_value(body: &[u8]) -> Value {
    serde_json::from_slice::<Value>(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).to_string()))
}

/// `{event: {headers, body, received_unix}, ...call_args}` -- the args a
/// run created from this event is dispatched with (AC1: "the tool having
/// received event.body").
fn build_run_args(call_args: &Value, body: &[u8], headers: &HeaderMap, allow: &[String]) -> Value {
    let mut args = if call_args.is_object() {
        call_args.clone()
    } else {
        json!({})
    };
    let event = json!({
        "headers": redacted_headers_json(headers, allow),
        "body": parse_body_value(body),
        "received_unix": now_unix(),
    });
    if let Some(obj) = args.as_object_mut() {
        obj.insert("event".to_string(), event);
    }
    args
}

// ---- POST /hooks/{namespace}/{tool} ----------------------------------------

struct HookAccepted {
    run_id: String,
}

fn status_for_hook_error(code: &str) -> StatusCode {
    match code {
        "hook_not_found" => StatusCode::NOT_FOUND,
        "signature_invalid" => StatusCode::UNAUTHORIZED,
        "events_rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "event_body_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::BAD_REQUEST,
    }
}

fn hook_error_response(err: AppError) -> Response {
    let code = err.code();
    let status = status_for_hook_error(code);
    let mut body = match &err {
        AppError::Structured { data, .. } if data.is_object() => data.clone(),
        _ => json!({}),
    };
    if let Some(obj) = body.as_object_mut() {
        obj.insert("error_code".to_string(), json!(code));
        obj.insert("error".to_string(), json!(err.to_string()));
    }
    (status, Json(body)).into_response()
}

/// Requirement 1's whole route: resolve tenant/tool/trigger, enforce the
/// body-size and rate ceilings, verify the signature, dedupe, enqueue.
async fn handle_hook(
    state: &AppState,
    namespace: &str,
    tool: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<HookAccepted, AppError> {
    let tenant = state
        .db
        .find_tenant_by_namespace(namespace.to_string())
        .await?
        .filter(|t| !t.disabled)
        .ok_or_else(hook_not_found)?;
    state
        .db
        .get_tool(tenant.id, tool.to_string())
        .await?
        .ok_or_else(hook_not_found)?;
    let triggers = state.db.list_triggers(tenant.id, Some(tool.to_string())).await?;
    let trigger = triggers
        .into_iter()
        .find(|t| t.kind == "event" && t.enabled)
        .ok_or_else(hook_not_found)?;

    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;

    if body.len() > plan.event_body_bytes_max as usize {
        return Err(AppError::Structured {
            code: "event_body_too_large",
            message: format!(
                "body is {} bytes, over the plan's {} byte limit",
                body.len(),
                plan.event_body_bytes_max
            ),
            data: json!({"limit_bytes": plan.event_body_bytes_max, "actual_bytes": body.len()}),
        });
    }

    if !state.event_rate_limiter.allow(&trigger.id, plan.events_per_minute) {
        return Err(AppError::Structured {
            code: "events_rate_limited",
            message: "too many events for this trigger in the last minute".to_string(),
            data: json!({"events_per_minute": plan.events_per_minute}),
        });
    }

    let config: Value = serde_json::from_str(&trigger.config_json).unwrap_or_else(|_| json!({}));
    let verify = config.get("verify").cloned().unwrap_or_else(|| json!({"scheme": "none"}));
    let secret = resolve_secret(state, tenant.id, &verify).await?;
    verify_event(&verify, secret.as_deref(), body, headers)?;

    let dedupe_header = config.get("dedupe_header").and_then(Value::as_str).map(str::to_string);
    let allow = allowed_headers(&verify, dedupe_header.as_deref());
    let call_args = config.get("args").cloned().unwrap_or_else(|| json!({}));
    let run_args = build_run_args(&call_args, body, headers, &allow);
    let args_json = serde_json::to_string(&run_args)
        .map_err(|e| AppError::Internal(format!("event args serialize: {e}")))?;

    // P1 requirement 7 / AC11: claim the dedupe key (if configured) before
    // inserting, so a duplicate delivery reuses the first run's id instead
    // of enqueuing a second one.
    let run_id = new_ulid();
    if let Some(header_name) = &dedupe_header
        && let Some(key) = headers.get(header_name).and_then(|v| v.to_str().ok())
    {
        if let Some(existing_run_id) = state
            .db
            .claim_event_dedupe(trigger.id.clone(), key.to_string(), run_id.clone())
            .await?
        {
            return Ok(HookAccepted { run_id: existing_run_id });
        }
    }

    state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            tool.to_string(),
            "event".to_string(),
            Some(trigger.id.clone()),
            None,
            plan.job_max_s,
            args_json,
            false,
            false,
        )
        .await?;
    let _ = state
        .db
        .update_trigger_after_fire(trigger.id.clone(), None, Some(run_id.clone()), now_unix())
        .await;
    Ok(HookAccepted { run_id })
}

/// `POST /hooks/{namespace}/{tool}` -- `http.rs` mounts this route.
pub async fn hook_receive(
    State(state): State<Arc<AppState>>,
    Path((namespace, tool)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    state.event_counters.record_received();
    match handle_hook(&state, &namespace, &tool, &headers, &body).await {
        Ok(accepted) => (StatusCode::ACCEPTED, Json(json!({"run_id": accepted.run_id}))).into_response(),
        Err(err) => {
            state.event_counters.record_rejected();
            hook_error_response(err)
        }
    }
}

// ---- host.trigger.test / host.trigger.replay -------------------------------

/// Builds a `HeaderMap` from a JSON object of string -> string, for
/// `host.trigger.test(headers: {...})` -- an MCP caller has no raw HTTP
/// headers of its own to hand in, only a JSON object shaped like one. An
/// entry whose name/value can't be a real header (non-ASCII, control
/// characters) is silently dropped rather than failing the whole call --
/// the same "best-effort, no silent 500" posture `redact_keys`' callers
/// already take with malformed input.
fn header_map_from_value(value: &Value) -> HeaderMap {
    let mut map = HeaderMap::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            let Some(s) = v.as_str() else { continue };
            let (Ok(name), Ok(val)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(s)) else {
                continue;
            };
            map.insert(name, val);
        }
    }
    map
}

/// `host.trigger.test(id, body, headers)` (P0 requirement 3 / AC7): drives
/// the exact same verify-then-enqueue path [`handle_hook`] does, without a
/// real HTTP request -- so a caller can prove its signature/payload works
/// before pointing a real sender at the URL, and see the same rejection
/// codes a live delivery would. The run it creates is marked `test: true`
/// (migration 0018) rather than `trigger='event'` alone, so it's
/// distinguishable from a live delivery in `host.runs.list`.
pub async fn test(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let row = state
        .db
        .get_trigger(tenant.id, id.clone())
        .await?
        .ok_or_else(|| crate::triggers::trigger_not_found(&id))?;
    if row.kind != "event" {
        return Err(trigger_invalid(
            "id",
            "host.trigger.test only supports an event ('kind: \"event\"') trigger".to_string(),
        ));
    }

    let body_value = args.get("body").cloned().unwrap_or_else(|| json!({}));
    let body_bytes = serde_json::to_vec(&body_value).unwrap_or_default();
    let headers = header_map_from_value(&args.get("headers").cloned().unwrap_or_else(|| json!({})));

    let config: Value = serde_json::from_str(&row.config_json).unwrap_or_else(|_| json!({}));
    let verify = config
        .get("verify")
        .cloned()
        .unwrap_or_else(|| json!({"scheme": "none", "allow_unverified": true}));
    let secret = resolve_secret(state, tenant.id, &verify).await?;
    verify_event(&verify, secret.as_deref(), &body_bytes, &headers)?;

    let dedupe_header = config.get("dedupe_header").and_then(Value::as_str).map(str::to_string);
    let allow = allowed_headers(&verify, dedupe_header.as_deref());
    let call_args = config.get("args").cloned().unwrap_or_else(|| json!({}));
    let run_args = build_run_args(&call_args, &body_bytes, &headers, &allow);
    let args_json = serde_json::to_string(&run_args)
        .map_err(|e| AppError::Internal(format!("event args serialize: {e}")))?;

    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let run_id = new_ulid();
    state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            row.tool_name.clone(),
            "event".to_string(),
            Some(row.id.clone()),
            None,
            plan.job_max_s,
            args_json,
            false,
            true,
        )
        .await?;
    Ok(json!({"run_id": run_id, "status": "queued", "test": true}))
}

/// `host.trigger.replay(run_id)` (P0 requirement 3 / AC8): re-enqueues an
/// event-triggered run's own stored `args_json` verbatim (no
/// re-verification -- the original delivery already passed it), with the
/// new run's `trigger_ref` naming the *original run* rather than the
/// trigger id (AC8's own wording), so `host.runs.get`/`list` shows exactly
/// which delivery this is a replay of.
pub async fn replay(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let run_id = arg_str(args, "run_id")?;
    let original = state
        .db
        .get_run(run_id.clone(), tenant.id)
        .await?
        .ok_or_else(|| run_not_found(&run_id))?;
    if original.trigger != "event" {
        return Err(trigger_invalid(
            "run_id",
            "only an event-triggered run can be replayed".to_string(),
        ));
    }
    let args_json = original.args_json.clone().unwrap_or_else(|| "{}".to_string());
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
            original.tool_name.clone(),
            "event".to_string(),
            Some(run_id.clone()),
            None,
            plan.job_max_s,
            args_json,
            false,
            false,
        )
        .await?;
    Ok(json!({"run_id": new_run_id, "status": "queued", "replay_of": run_id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        map
    }

    #[test]
    fn hmac_sha256_matches_billing_helper_for_the_same_inputs() {
        // Sanity check that `hmac_hex`'s reuse of `billing::hmac_sha256`
        // wires up correctly, not a re-derivation of HMAC's own test
        // vectors (`billing.rs` already has those).
        let a = hmac_hex("hmac-sha256", b"secret", b"payload");
        let b = to_hex(&crate::billing::hmac_sha256(b"secret", b"payload"));
        assert_eq!(a, b);
    }

    #[test]
    fn verify_event_none_scheme_always_passes() {
        let verify = json!({"scheme": "none", "allow_unverified": true});
        assert!(verify_event(&verify, None, b"{}", &HeaderMap::new()).is_ok());
    }

    #[test]
    fn verify_event_hmac_sha256_accepts_correct_and_rejects_wrong_signature() {
        let body = b"hello world";
        let sig = hmac_hex("hmac-sha256", b"s3cr3t", body);
        let verify = json!({"scheme": "hmac-sha256", "header": "X-Sig", "secret": "s3cr3t"});
        let good = headers(&[("X-Sig", &sig)]);
        assert!(verify_event(&verify, Some("s3cr3t"), body, &good).is_ok());

        let bad = headers(&[("X-Sig", "deadbeef")]);
        let err = verify_event(&verify, Some("s3cr3t"), body, &bad).unwrap_err();
        assert_eq!(err.code(), "signature_invalid");
    }

    #[test]
    fn verify_event_hmac_sha256_honors_a_prefix() {
        let body = b"payload";
        let sig = hmac_hex("hmac-sha256", b"k", body);
        let verify = json!({"scheme": "hmac-sha256", "header": "X-Hub-Signature-256", "secret": "k", "prefix": "sha256="});
        let h = headers(&[("X-Hub-Signature-256", &format!("sha256={sig}"))]);
        assert!(verify_event(&verify, Some("k"), body, &h).is_ok());
    }

    #[test]
    fn verify_event_hmac_sha1_matches_github_legacy_style() {
        let body = b"legacy";
        let sig = hmac_hex("hmac-sha1", b"k", body);
        let verify = json!({"scheme": "hmac-sha1", "header": "X-Hub-Signature", "secret": "k", "prefix": "sha1="});
        let h = headers(&[("X-Hub-Signature", &format!("sha1={sig}"))]);
        assert!(verify_event(&verify, Some("k"), body, &h).is_ok());
    }

    #[test]
    fn verify_event_token_scheme_is_a_plain_shared_secret_header() {
        let verify = json!({"scheme": "token", "header": "X-Token", "secret": "shh"});
        let good = headers(&[("X-Token", "shh")]);
        assert!(verify_event(&verify, Some("shh"), b"{}", &good).is_ok());
        let bad = headers(&[("X-Token", "nope")]);
        assert!(verify_event(&verify, Some("shh"), b"{}", &bad).is_err());
    }

    /// AC3: a Stripe-style `t=,v1=` signature with a timestamp older than
    /// the tolerance fails with `reason: "timestamp"`, not a generic
    /// mismatch.
    #[test]
    fn verify_event_stripe_style_stale_timestamp_names_the_reason() {
        let body = b"{}";
        let old_t = now_unix() - 10_000;
        let sig = hmac_hex("hmac-sha256", b"whsec", &[old_t.to_string().as_bytes(), b".", body].concat());
        let verify = json!({
            "scheme": "hmac-sha256",
            "header": "Stripe-Signature",
            "secret": "whsec",
            "timestamp_header": "Stripe-Signature",
            "tolerance_s": 300,
        });
        let h = headers(&[("Stripe-Signature", &format!("t={old_t},v1={sig}"))]);
        let err = verify_event(&verify, Some("whsec"), body, &h).unwrap_err();
        assert_eq!(err.code(), "signature_invalid");
        let AppError::Structured { data, .. } = err else { panic!("expected Structured") };
        assert_eq!(data["reason"], json!("timestamp"));
    }

    #[test]
    fn verify_event_stripe_style_accepts_a_fresh_valid_signature() {
        let body = b"{}";
        let t = now_unix();
        let sig = hmac_hex("hmac-sha256", b"whsec", &[t.to_string().as_bytes(), b".", body].concat());
        let verify = json!({
            "scheme": "hmac-sha256",
            "header": "Stripe-Signature",
            "secret": "whsec",
            "timestamp_header": "Stripe-Signature",
        });
        let h = headers(&[("Stripe-Signature", &format!("t={t},v1={sig}"))]);
        assert!(verify_event(&verify, Some("whsec"), body, &h).is_ok());
    }

    #[test]
    fn validate_verify_config_rejects_none_without_the_flag() {
        let err = validate_verify_config(&json!({"scheme": "none"})).unwrap_err();
        assert_eq!(err.code(), "trigger_invalid");
    }

    #[test]
    fn validate_verify_config_accepts_none_with_the_flag() {
        let out = validate_verify_config(&json!({"scheme": "none", "allow_unverified": true})).unwrap();
        assert_eq!(out["scheme"], json!("none"));
        assert_eq!(out["allow_unverified"], json!(true));
    }

    #[test]
    fn validate_verify_config_rejects_unknown_scheme() {
        let err = validate_verify_config(&json!({"scheme": "md5"})).unwrap_err();
        assert_eq!(err.code(), "trigger_invalid");
    }

    #[test]
    fn validate_verify_config_requires_header_and_secret_for_hmac() {
        assert!(validate_verify_config(&json!({"scheme": "hmac-sha256"})).is_err());
        assert!(validate_verify_config(&json!({"scheme": "hmac-sha256", "header": "X"})).is_err());
        assert!(validate_verify_config(&json!({"scheme": "hmac-sha256", "header": "X", "secret": "s"})).is_ok());
    }

    #[test]
    fn allowed_headers_includes_base_set_and_config_named_ones() {
        let verify = json!({"scheme": "hmac-sha256", "header": "X-Hub-Signature-256"});
        let allow = allowed_headers(&verify, Some("X-GitHub-Delivery"));
        assert!(allow.contains(&"content-type".to_string()));
        assert!(allow.contains(&"user-agent".to_string()));
        assert!(allow.contains(&"x-request-id".to_string()));
        assert!(allow.contains(&"x-hub-signature-256".to_string()));
        assert!(allow.contains(&"x-github-delivery".to_string()));
    }

    #[test]
    fn redacted_headers_json_drops_anything_not_allowlisted() {
        let h = headers(&[("Content-Type", "application/json"), ("X-Secret-Debug", "leak-me")]);
        let allow = vec!["content-type".to_string()];
        let redacted = redacted_headers_json(&h, &allow);
        assert_eq!(redacted["content-type"], json!("application/json"));
        assert!(redacted.get("x-secret-debug").is_none());
    }

    #[test]
    fn parse_body_value_parses_json_and_falls_back_to_string() {
        assert_eq!(parse_body_value(br#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(parse_body_value(b"not json"), json!("not json"));
    }

    #[test]
    fn event_counters_prunes_and_counts_within_the_window() {
        let counters = EventCounters::new();
        assert_eq!(counters.received_1h(), 0);
        counters.record_received();
        counters.record_received();
        counters.record_rejected();
        assert_eq!(counters.received_1h(), 2);
        assert_eq!(counters.rejected_1h(), 1);
    }

    #[test]
    fn event_rate_limiter_allows_up_to_the_limit_then_blocks() {
        let limiter = EventRateLimiter::new();
        for i in 0..5 {
            assert!(limiter.allow("trig-1", 5), "event {i} within the limit must be allowed");
        }
        assert!(!limiter.allow("trig-1", 5), "the 6th event in the window must be refused");
        assert!(limiter.allow("trig-2", 5), "a different trigger's own window must be independent");
    }
}
