//! The axum app: `GET /healthz` (unauthenticated) and `POST /mcp` (the
//! `rmcp` streamable-HTTP service, stateless per the 2026-07-28
//! specification).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;

use crate::handler::McpHostHandler;
use crate::state::{AppState, MAX_REQUEST_BODY_BYTES};

const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

async fn healthz(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_ok = state.db.is_writable().await;
    let (tools_total, tenants_total) = state.db.counts().await.unwrap_or((0, 0));
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "db_ok": db_ok,
        "tools_total": tools_total,
        "tenants_total": tenants_total,
    }))
}

/// Ensures every `/mcp` response carries `MCP-Protocol-Version` (AC1), and
/// emits one structured request-line log entry.
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

    if !response.headers().contains_key("MCP-Protocol-Version") {
        response.headers_mut().insert(
            "MCP-Protocol-Version",
            HeaderValue::from_static(MCP_PROTOCOL_VERSION),
        );
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
