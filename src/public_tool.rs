//! PRD-mcphost-public-tool-url: `host.tool_share(visibility = "url")`'s
//! own token mint/reuse (requirement 1) plus the `/x/{token}/{tool}` HTTP
//! route family (requirement 2-6, 8-9, 11) -- a plain-HTTPS ingress to a
//! published tool that needs no MCP client, key, or account, governed by
//! the same share caller limit and tenant run budget `host.tool_call`
//! already is.
//!
//! `sharing::tool_share`/`tool_unshare` call into this module to mint or
//! revoke a token; `http.rs` mounts the route handlers this module
//! exports.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::db::{RunRow, Tenant, ToolRow};
use crate::errors::AppError;
use crate::state::AppState;

/// `https://<host>/x/<token>/<tool>` (AC1's own regex) -- same
/// `public_url.trim_end_matches('/')` convention [`crate::webhooks::webhook_url`]
/// already uses, so a public tool URL never disagrees with where every
/// other generated link on this host points.
pub fn public_tool_url(state: &AppState, token: &str, tool: &str) -> String {
    format!("{}/x/{}/{}", state.public_url.trim_end_matches('/'), token, tool)
}

/// `host.tool_share(name, visibility = "url")`'s own mint-or-reuse step
/// (AC1): an existing token row for `(tenant.id, name)` is decrypted and
/// its URL re-returned unchanged (requirement 1's "re-sharing returns the
/// same URL until revoked"); otherwise a fresh 26-character token is
/// minted, hashed (for the request-time lookup) and encrypted (so it can
/// still be re-displayed on the next `host.tool_share` call -- unlike a
/// webhook trigger's secret, this one is never shown-once-and-forgotten).
pub async fn mint_or_reuse_url_share(state: &AppState, tenant: &Tenant, name: &str) -> Result<String, AppError> {
    if let Some((_, token_enc, token_nonce)) = state.db.find_url_share(tenant.id, name.to_string()).await? {
        let token = state.secrets.decrypt(&token_enc, &token_nonce)?;
        return Ok(public_tool_url(state, &token, name));
    }
    let token = crate::auth::generate_public_token();
    let token_hash = crate::auth::hash_key(&token);
    let (token_enc, token_nonce) = state.secrets.encrypt(&token)?;
    let id = crate::state::new_ulid();
    state
        .db
        .insert_url_share(
            id,
            tenant.id,
            name.to_string(),
            token_hash,
            token_enc,
            token_nonce,
            crate::state::now_unix(),
        )
        .await?;
    Ok(public_tool_url(state, &token, name))
}

/// `host.tool_unshare`/a `host.tool_share` to any other visibility (AC4,
/// AC8): deletes the token row if one exists. Best-effort by design --
/// callers that already hold `tenant`/`name` from a successful unshare
/// never need to branch on this; there is nothing more to revoke if no
/// token was ever minted.
pub async fn revoke_url_share(state: &AppState, tenant_id: i64, name: &str) -> Result<(), AppError> {
    state.db.delete_url_share(tenant_id, name.to_string()).await
}

// ---- GET/POST /x/{token}/{tool} ----------------------------------------

/// A revoked token (its row deleted) and a token that was never issued
/// resolve through the exact same branch below -- requirement 5 / AC4's
/// own constant-time guarantee comes from there being only one "no row"
/// code path, not from a timing-sensitive workaround.
enum ShareError {
    NotFound,
    TenantPaused,
}

struct ResolvedShare {
    tenant: Tenant,
    tool_row: ToolRow,
    token_id: String,
}

/// The one indexed lookup (requirement 2/5/9) every `/x/{token}/{tool}...`
/// route resolves a token through: hash -> `(tenant, tool)` row ->
/// tenant not paused -> tool still `visibility = "url"`. Any failure at any
/// step is [`ShareError::NotFound`] except an explicitly paused tenant
/// (AC7's own 503), so a caller can never distinguish "wrong token" from
/// "right token, wrong tool" from "token revoked" from "tool no longer
/// shared".
async fn resolve_share(state: &AppState, token: &str, tool: &str) -> Result<ResolvedShare, ShareError> {
    let hash = crate::auth::hash_key(token);
    let Ok(Some((tenant_id, tool_name, token_id))) = state.db.find_url_share_by_hash(hash).await else {
        return Err(ShareError::NotFound);
    };
    if tool_name != tool {
        return Err(ShareError::NotFound);
    }
    let Ok(Some(tenant)) = state.db.find_tenant_by_id(tenant_id).await else {
        return Err(ShareError::NotFound);
    };
    // PRD-mcphost-public-tool-url requirement 9 (AC7): the admin kill
    // switch (`admin.tenant_disable`, the same `tenants.disabled` flag
    // every other call path already honors) stops public URL calls with
    // 503, distinct from the 404 a revoked/unknown token gets.
    if tenant.disabled {
        return Err(ShareError::TenantPaused);
    }
    let Ok(Some(tool_row)) = state.db.get_tool(tenant_id, tool_name.clone()).await else {
        return Err(ShareError::NotFound);
    };
    if tool_row.visibility != "url" {
        return Err(ShareError::NotFound);
    }
    Ok(ResolvedShare { tenant, tool_row, token_id })
}

/// AC4: an unknown or revoked token answers with an empty body -- same
/// "leak nothing about why it failed" convention
/// [`crate::webhooks::webhook_error_response`] already uses for its own
/// opaque-id route.
fn not_found_response() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

fn tenant_paused_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"ok": false, "error": {"code": "tenant_paused", "message": "this tenant is currently paused"}})),
    )
        .into_response()
}

fn error_envelope(status: StatusCode, code: &str, message: String) -> Response {
    (status, Json(json!({"ok": false, "error": {"code": code, "message": message}}))).into_response()
}

/// Same header-then-peer-fallback shape as [`crate::claim`]'s own
/// `source_ip` -- this route sits on the same axum `Router` behind the
/// same reverse proxy, so it needs the same `X-Forwarded-For` handling to
/// get a real per-caller address instead of Caddy's own loopback one.
fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> String {
    let forwarded_for = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    crate::state::resolve_source_ip(Some(&peer.ip().to_string()), forwarded_for)
}

/// Requirement 6 (AC5): 429 with a `Retry-After` header -- checked before
/// [`resolve_share`] so a flood of requests never pays for the token
/// lookup either, and (requirement's own "no run created") strictly before
/// [`dispatch`] ever enqueues anything.
fn rate_limited_response() -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"ok": false, "error": {"code": "rate_limited", "message": "too many calls from this address; try again shortly"}})),
    )
        .into_response();
    response
        .headers_mut()
        .insert("Retry-After", HeaderValue::from_static("60"));
    response
}

/// Requirement 3's error envelope ("the same error envelope as
/// PRD-mcphost-first-hour-support-surface"): built from the real
/// [`AppError`] via [`AppError::into_error_data_at`] (the same sanitizing/
/// logging/`help_url`-attaching machinery the JSON-RPC error path already
/// uses), re-shaped into this route's plain `{ok, error}` envelope instead
/// of a JSON-RPC error object.
fn app_error_response(state: &AppState, err: AppError) -> Response {
    let status = match err.code() {
        "args_invalid" => StatusCode::BAD_REQUEST,
        "tool_not_found" => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let data = err.into_error_data_at(Some(&state.public_url));
    let error_code = data
        .data
        .as_ref()
        .and_then(|d| d.get("error_code"))
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    let help_url = data.data.as_ref().and_then(|d| d.get("help_url")).cloned();
    let mut obj = json!({"code": error_code, "message": data.message.to_string()});
    if let Some(help_url) = help_url
        && let Some(obj) = obj.as_object_mut()
    {
        obj.insert("help_url".to_string(), help_url);
    }
    (status, Json(json!({"ok": false, "error": obj}))).into_response()
}

const TERMINAL_STATUSES: &[&str] = &["done", "error", "timeout", "cancelled"];

/// Requirement 3 (AC2/AC3/AC6): waits up to
/// [`AppState::public_url_sync_deadline`] for `run_id` to reach a terminal
/// status, polling the same way [`crate::runs::wait`] does; past the
/// deadline it answers `202 {run_id, poll}` instead (AC6) so the caller
/// never blocks past the threshold even though the run keeps executing in
/// the background (the real work already queued through
/// [`crate::runs::enqueue_url`] -- this function only decides how long to
/// wait for it before answering).
async fn poll_and_respond(state: &AppState, tenant: &Tenant, run_id: &str, token: &str, tool_name: &str) -> Response {
    let deadline = Instant::now() + state.public_url_sync_deadline;
    loop {
        let run = match state.db.get_run(run_id.to_string(), tenant.id).await {
            Ok(Some(r)) => r,
            Ok(None) => return error_envelope(StatusCode::INTERNAL_SERVER_ERROR, "internal", "run vanished".to_string()),
            Err(_) => return error_envelope(StatusCode::INTERNAL_SERVER_ERROR, "storage", "storage error".to_string()),
        };
        if TERMINAL_STATUSES.contains(&run.status.as_str()) {
            return terminal_response(state, tenant, &run, run_id).await;
        }
        if Instant::now() >= deadline {
            let poll = format!("{}/runs/{}", public_tool_url(state, token, tool_name), run_id);
            return (
                StatusCode::ACCEPTED,
                Json(json!({"ok": true, "run_id": run_id, "poll": poll})),
            )
                .into_response();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// `run`'s own terminal state, shaped as this route's wire response
/// (requirement 3): `200 {ok: true, result, run_id}` for `done`, a 5xx
/// `{ok: false, error}` for anything else.
async fn terminal_response(state: &AppState, tenant: &Tenant, run: &RunRow, run_id: &str) -> Response {
    if run.status != "done" {
        let code = run.error_class.clone().unwrap_or_else(|| "run_failed".to_string());
        return error_envelope(
            StatusCode::INTERNAL_SERVER_ERROR,
            &code,
            format!("run '{run_id}' finished with status '{}'", run.status),
        );
    }
    let merged = match crate::runs::attach_result(state, tenant.id, run, json!({})).await {
        Ok(v) => v,
        Err(_) => return error_envelope(StatusCode::INTERNAL_SERVER_ERROR, "storage", "storage error".to_string()),
    };
    (
        StatusCode::OK,
        Json(json!({"ok": true, "result": merged["result"], "run_id": run_id})),
    )
        .into_response()
}

/// Shared by [`x_post`] (and, once added, the `GET` handler): queues the
/// run (requirement 2) and waits for it per [`poll_and_respond`].
async fn dispatch(state: &AppState, token: &str, share: ResolvedShare, args: Value) -> Response {
    let tool_name = share.tool_row.name.clone();
    let run_id = match crate::runs::enqueue_url(state, &share.tenant, &tool_name, args, &share.token_id).await {
        Ok(id) => id,
        Err(e) => return app_error_response(state, e),
    };
    poll_and_respond(state, &share.tenant, &run_id, token, &tool_name).await
}

// ---- CORS (requirement 11, AC10) ----------------------------------------

/// Requirement 11 (AC10): `Access-Control-Allow-Origin: *` on every
/// response from this route family, so a browser page on another origin
/// can read a `fetch`'s result -- a public tool URL's whole point is "no
/// MCP client, key, or account", which includes no same-origin
/// requirement either. Applied as route-scoped middleware (`route_layer`
/// in `http.rs`, not `.layer()` on the whole router) rather than inside
/// each handler, so [`x_get`]/[`x_post`]/[`x_poll`]/[`x_options`] -- and
/// every early return inside them (404, 429, 503, ...) -- all get it for
/// free.
pub async fn add_cors_header(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

/// `OPTIONS /x/{token}/{tool}` (requirement 11, AC10): the CORS preflight
/// -- `204` with the allow-list a browser's own preflight check reads,
/// independent of whether `token`/`tool` resolve to anything (a preflight
/// carries no credential a real call would need to check anyway).
pub async fn x_options() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS"),
            (header::ACCESS_CONTROL_ALLOW_HEADERS, "content-type"),
        ],
    )
        .into_response()
}

/// `GET /x/{token}/{tool}/runs/{run_id}` (requirement 4, AC6): the poll
/// side of a `202` answer. Scoped to runs started through THIS token
/// only (`run.trigger_ref == share.token_id`) -- a run id guessed or
/// leaked from elsewhere in this tenant's own ledger (e.g. an ordinary
/// `host.tool_call` run) does not resolve here, same non-leaking posture
/// [`resolve_share`] already gives the main route.
pub async fn x_poll(
    State(state): State<Arc<AppState>>,
    Path((token, tool, run_id)): Path<(String, String, String)>,
) -> Response {
    let share = match resolve_share(&state, &token, &tool).await {
        Ok(share) => share,
        Err(ShareError::NotFound) => return not_found_response(),
        Err(ShareError::TenantPaused) => return tenant_paused_response(),
    };
    let run = match state.db.get_run(run_id.clone(), share.tenant.id).await {
        Ok(Some(run)) => run,
        Ok(None) => return not_found_response(),
        Err(_) => return error_envelope(StatusCode::INTERNAL_SERVER_ERROR, "storage", "storage error".to_string()),
    };
    if run.trigger_ref.as_deref() != Some(share.token_id.as_str()) {
        return not_found_response();
    }
    if TERMINAL_STATUSES.contains(&run.status.as_str()) {
        return terminal_response(&state, &share.tenant, &run, &run_id).await;
    }
    let poll = format!("{}/runs/{}", public_tool_url(&state, &token, &share.tool_row.name), run_id);
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "run_id": run_id, "poll": poll})),
    )
        .into_response()
}

/// `POST /x/{token}/{tool}` (requirement 2, AC2): the body (parsed as
/// JSON regardless of `Content-Type` -- the PRD's own non-goal list rules
/// out anything beyond "a plain HTTPS endpoint", so a lenient parse here
/// is simpler than a second rejection path for a wrong/missing header) is
/// the tool's arguments.
pub async fn x_post(
    State(state): State<Arc<AppState>>,
    Path((token, tool)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state
        .url_rate_limiter
        .allow(&client_ip(&headers, peer), crate::state::PUBLIC_URL_RATE_LIMIT_PER_MINUTE)
    {
        return rate_limited_response();
    }
    let share = match resolve_share(&state, &token, &tool).await {
        Ok(share) => share,
        Err(ShareError::NotFound) => return not_found_response(),
        Err(ShareError::TenantPaused) => return tenant_paused_response(),
    };
    let args: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_envelope(
                StatusCode::BAD_REQUEST,
                "args_invalid",
                format!("request body must be valid JSON: {e}"),
            );
        }
    };
    dispatch(&state, &token, share, args).await
}

/// Requirement 2's own query-string coercion: "each value parsed as JSON
/// when it parses, else a string" -- `n=3` becomes the number `3`,
/// `text=hi` stays the string `"hi"` (not valid JSON on its own), and
/// `flag=true`/`obj={"a":1}` parse as their real JSON types too. A caller
/// that needs anything this simple-minded rule can't express (typed
/// arrays, nesting) is pointed at `POST` instead by the technical
/// considerations section -- not this route's job to get right.
fn query_to_args(params: HashMap<String, String>) -> Value {
    let mut obj = serde_json::Map::with_capacity(params.len());
    for (k, v) in params {
        let value = serde_json::from_str::<Value>(&v).unwrap_or(Value::String(v));
        obj.insert(k, value);
    }
    Value::Object(obj)
}

/// `GET /x/{token}/{tool}` (requirement 2, AC3): the query string is the
/// tool's arguments, coerced per [`query_to_args`].
pub async fn x_get(
    State(state): State<Arc<AppState>>,
    Path((token, tool)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if !state
        .url_rate_limiter
        .allow(&client_ip(&headers, peer), crate::state::PUBLIC_URL_RATE_LIMIT_PER_MINUTE)
    {
        return rate_limited_response();
    }
    let share = match resolve_share(&state, &token, &tool).await {
        Ok(share) => share,
        Err(ShareError::NotFound) => return not_found_response(),
        Err(ShareError::TenantPaused) => return tenant_paused_response(),
    };
    let args = query_to_args(params);
    dispatch(&state, &token, share, args).await
}
