//! PRD-mcphost-sse-frame-contract-generated AC4 (P0) -- Given a handler that
//! contract: frames::emit panics in debug on a frame kind the request type disallows
//! pushes a `list_changed` notification inside a `tools/list` handler, When
//! it runs in debug, Then `frames::emit` panics with the request type and the
//! disallowed frame kind.
//!
//! A real `rmcp` streamable-HTTP service serves a probe handler whose
//! `list_tools` hands the request's own `Peer` to `frames::emit`; the emit
//! runs in a spawned task so its panic comes back as a `JoinError` the
//! handler reports over a channel.

use mcphost::protocol::frames::{Frame, emit};
use rmcp::model::{ListToolsResult, PaginatedRequestParams, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
struct StrayNotifier {
    panics: mpsc::UnboundedSender<String>,
}

impl ServerHandler for StrayNotifier {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let peer = ctx.peer.clone();
        let outcome = tokio::spawn(async move { emit(&peer, Frame::ToolListChanged, "tools/list").await }).await;
        let message = match outcome {
            Err(join) if join.is_panic() => {
                let payload = join.into_panic();
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default()
            }
            other => format!("emit did not panic: {other:?}"),
        };
        let _ = self.panics.send(message);
        Ok(ListToolsResult::with_all_items(Vec::new()))
    }
}

#[tokio::test]
async fn emit_of_a_disallowed_frame_panics_naming_request_type_and_kind() {
    const { assert!(cfg!(debug_assertions), "AC4 is a debug-build contract; run it without --release") };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let config = StreamableHttpServerConfig::default().with_json_response(true).with_legacy_session_mode(false).disable_allowed_hosts();
    let service = StreamableHttpService::new(move || Ok(StrayNotifier { panics: tx.clone() }), Arc::new(LocalSessionManager::default()), config);
    let app = axum::Router::new().route_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "sfc-ac04", "version": "0.1.0"},
            }},
        }))
        .send()
        .await
        .expect("tools/list request");
    assert!(resp.status().is_success(), "tools/list status {}", resp.status());

    let message = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("handler reported within 10 s")
        .expect("handler channel open");
    assert!(message.contains("tools/list"), "panic must name the request type: {message}");
    assert!(message.contains("ListChanged"), "panic must name the disallowed frame kind: {message}");
}
