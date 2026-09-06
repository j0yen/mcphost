//! The axum app: `GET /healthz` (unauthenticated) and `POST /mcp` (the
//! `rmcp` stateless streamable-HTTP service, advertising whatever
//! protocol version this build of `rmcp` actually negotiates -- see
//! [`advertised_protocol_version`].

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rmcp::model::ProtocolVersion;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;

use crate::handler::McpHostHandler;
use crate::state::{AppState, MAX_REQUEST_BODY_BYTES};

/// The `MCP-Protocol-Version` value this server advertises, derived from
/// `rmcp::model::ProtocolVersion::LATEST` rather than a string literal
/// (PRD-mcphost-protocol-compat requirement 6): the version this
/// middleware stamps onto responses must always be the version `rmcp`
/// actually negotiates, so an `rmcp` upgrade that moves `LATEST` either
/// moves this advertisement with it or fails requirement 7's test. The
/// `HeaderValue` is computed once and cached, since `HeaderValue::from_str`
/// is fallible and `HeaderValue::from_static` needs a `&'static str` we
/// don't have at compile time.
fn advertised_protocol_version() -> Option<&'static HeaderValue> {
    static VALUE: OnceLock<Option<HeaderValue>> = OnceLock::new();
    VALUE
        .get_or_init(|| HeaderValue::from_str(ProtocolVersion::LATEST.as_str()).ok())
        .as_ref()
}

async fn healthz(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_ok = state.db.is_writable().await;
    let (tools_total, tenants_total) = state.db.counts().await.unwrap_or((0, 0));
    // PRD-mcphost-tenant-delete requirement 5 / AC9: `tenants_probe` is
    // additive -- `tenants_total` (read by `mcphost-deploy probe` and the
    // measure job today) is unchanged, so `tenants_total - tenants_probe`
    // is the real-tenant count.
    let tenants_probe = state.db.probe_tenant_count().await.unwrap_or(0);
    let mut body = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "db_ok": db_ok,
        "tools_total": tools_total,
        "tenants_total": tenants_total,
        "tenants_probe": tenants_probe,
        "sandbox_mechanism": state.sandbox_mechanism,
    });
    // PRD-mcphost-sandbox-ready requirement 2: additive fields, populated
    // from an actual sandboxed-process probe rather than the `on_path`
    // check `sandbox_mechanism` above has always been. `None` (absent
    // fields) when no registered kind has a self-test to report -- an
    // `echo`/`http`-only deployment is unaffected (migration note:
    // "existing clients that ignore unknown fields are unaffected").
    if let Some(status) = state.kinds.all().find_map(|k| k.sandbox_status())
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("sandbox_ready".to_string(), json!(status.ready));
        obj.insert("sandbox_detail".to_string(), json!(status.detail));
        obj.insert("sandbox_checked_at".to_string(), json!(status.checked_at));
    }
    // PRD-grand-loop-billing AC4/AC11: `off` with no Stripe key, else
    // `test`/`live` from the key prefix, plus a live count of tenants on
    // any paid plan -- additive fields, ignored by an old client the same
    // way the sandbox fields above are.
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "billing_mode".to_string(),
            json!(state.billing_config.billing_mode()),
        );
        obj.insert(
            "paying_tenants".to_string(),
            json!(state.db.count_paying_tenants().await.unwrap_or(0)),
        );
    }
    // PRD-mcphost-metered-overage AC8: `meter_lag` only appears when
    // metering is configured (`metered_price_id` set) -- unconfigured, the
    // key is absent entirely, not present-and-zero, so a caller can
    // distinguish "no overage billing on this host" from "0 calls behind."
    if state.billing_config.metered_price_id.is_some()
        && let Some(obj) = body.as_object_mut()
    {
        let (last_call_id, _) = state.db.get_meter_state().await.unwrap_or((0, None));
        let lag = state.db.meter_lag(last_call_id).await.unwrap_or(0);
        obj.insert("meter_lag".to_string(), json!(lag));
    }
    Json(body)
}

/// `POST /billing/webhook` (AC6/AC7/AC8/AC10): verifies `Stripe-Signature`
/// and applies the event via `billing::process_webhook`. Deliberately
/// takes the raw `Bytes` body (not a `Json<Value>` extractor) because
/// signature verification is over the *exact* bytes Stripe sent, before
/// any re-serialization could change them.
async fn billing_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let signature = headers
        .get("Stripe-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    match crate::billing::process_webhook(&state, &body, signature).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            let data = err.into_error_data();
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error_code": "invalid_webhook_signature", "error": data.message})),
            )
                .into_response()
        }
    }
}

/// `GET /billing/done` / `GET /billing/cancel` (requirement: "plain text
/// pages"): the `success_url`/`cancel_url` a Checkout Session redirects a
/// human's browser to after they finish (or abandon) payment. This host
/// has no UI (Non-goals: "A pricing page or any UI"), so these are the
/// whole experience -- a short, honest sentence, not a redirect back into
/// the MCP surface a browser can't call anyway.
async fn billing_done() -> impl IntoResponse {
    (
        [("Content-Type", "text/plain; charset=utf-8")],
        "Payment received. Ask your agent to call billing.status to confirm the upgrade.",
    )
}

async fn billing_cancel() -> impl IntoResponse {
    (
        [("Content-Type", "text/plain; charset=utf-8")],
        "Checkout canceled. No changes were made to your plan.",
    )
}

/// AC19: `GET /.well-known/mcp/<namespace>/server.json`, unauthenticated
/// (this is a public discovery document by design, same as the registry
/// entry it mirrors). 404 if that tenant has never run
/// `host.registry_publish` successfully.
async fn well_known_server_json(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> impl IntoResponse {
    match state.db.get_registry_document(namespace).await {
        Ok(Some(doc)) => (StatusCode::OK, Json(doc)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error_code": "tool_not_found", "error": "no published server.json for this namespace"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error_code": "storage", "error": "storage error"})),
        )
            .into_response(),
    }
}

/// Ensures every response carries `MCP-Protocol-Version` (AC1), and emits
/// one structured request-line log entry. This layer is attached to the
/// whole router (see `build_router` below), not just `/mcp` -- `/healthz`
/// and `/.well-known/mcp/{ns}/server.json` get the header and the log line
/// too. That is deliberate: it keeps one middleware as the single source
/// of the request log, and an unauthenticated health/discovery endpoint
/// carrying an accurate protocol-version header is harmless.
async fn protocol_version_and_log(req: Request<Body>, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let mcp_name = req
        .headers()
        .get("Mcp-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let mut response = next.run(req).await;

    if !response.headers().contains_key("MCP-Protocol-Version")
        && let Some(value) = advertised_protocol_version()
    {
        response
            .headers_mut()
            .insert("MCP-Protocol-Version", value.clone());
    }

    let duration_ms = start.elapsed().as_millis();
    tracing::info!(
        method = %method,
        path = %path,
        mcp_name = mcp_name.as_deref().unwrap_or(""),
        duration_ms,
        status = response.status().as_u16(),
        "http request"
    );

    response
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_json_response(true)
        .with_legacy_session_mode(false)
        .with_max_request_body_bytes(MAX_REQUEST_BODY_BYTES)
        // This endpoint's security boundary is the bearer key, not the Host
        // header: it is meant to be reached at whatever public domain the
        // operator maps to it (see the PRD's "Public domain and TLS name"
        // open question), often behind a reverse proxy. DNS-rebinding
        // protection via allowed_hosts is the right default for a
        // loopback dev server; it is not a substitute for auth here, so we
        // disable it rather than requiring the operator to enumerate every
        // hostname up front.
        .disable_allowed_hosts();

    let mcp_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(McpHostHandler::new(mcp_state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/.well-known/mcp/{namespace}/server.json",
            get(well_known_server_json),
        )
        .route("/billing/webhook", post(billing_webhook))
        .route("/billing/done", get(billing_done))
        .route("/billing/cancel", get(billing_cancel))
        .route_service("/mcp", service)
        .layer(middleware::from_fn(protocol_version_and_log))
        .with_state(state)
}

/// Serve on an already-bound listener. Split out from [`serve`] so
/// integration tests can bind to `127.0.0.1:0`, read back the assigned
/// port, and hand the listener here without a bind-then-race.
pub async fn serve_on_listener(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    let app = build_router(state);
    tracing::info!(addr = ?listener.local_addr().ok(), "mcphost listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

pub async fn serve(bind: SocketAddr, state: Arc<AppState>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    serve_on_listener(listener, state).await
}
