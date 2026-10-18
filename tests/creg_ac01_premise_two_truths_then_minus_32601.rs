//! AC1 (P0) -- PRD-mcphost-server-capabilities-from-registry. The premise: a
//! 0.107.0-shaped server (no registry, rmcp defaults) advertises only
//! `tools` yet answers `resources/list`, `prompts/list` and
//! `resources/templates/list` with a `result`. With the registry wired the
//! same unimplemented rows answer `-32601` with `data.capability` naming the row.

use crate::common;
use common::{McpClient, TestServer};
use rmcp::ServerHandler;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;
use std::sync::Arc;

const METHODS: [(&str, &str); 3] = [
    ("resources/list", "resources"),
    ("prompts/list", "prompts"),
    ("resources/templates/list", "resources"),
];

/// What 0.107.0 shipped: tools advertised, every other hook rmcp's default.
#[derive(Clone)]
struct StockHandler;

impl ServerHandler for StockHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_tool_list_changed().build())
    }
}

async fn start_stock_server() -> String {
    let config = StreamableHttpServerConfig::default()
        .with_json_response(true)
        .with_legacy_session_mode(false)
        .disable_allowed_hosts();
    let service = StreamableHttpService::new(|| Ok(StockHandler), Arc::new(LocalSessionManager::default()), config);
    let router = axum::Router::new().route_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn premise_holds_on_a_stock_server_and_the_registry_closes_it() {
    // Premise: rmcp defaults answer a result for unadvertised capabilities.
    let stock = McpClient::new(&start_stock_server().await);
    let init = stock.initialize().await;
    let caps = &init["result"]["capabilities"];
    assert!(caps.get("resources").is_none(), "stock server must not advertise resources: {caps}");
    assert!(caps.get("prompts").is_none(), "stock server must not advertise prompts: {caps}");
    for (method, _) in METHODS {
        let reply = stock.rpc(method, json!({})).await;
        assert!(
            reply.get("result").is_some(),
            "premise: stock server must answer {method} with a result, got {reply}"
        );
    }

    // Registry wired: the same three are -32601 naming the row.
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let init = client.initialize().await;
    let caps = &init["result"]["capabilities"];
    // `resources` (PRD-mcphost-resources-for-tenant-data) and `prompts`
    // (PRD-mcphost-prompts-from-published-tools) are implemented: both are
    // advertised and answer a result, so only unimplemented rows answer -32601.
    assert!(caps.get("resources").is_some() && caps.get("prompts").is_some(), "{caps}");
    for (method, row) in METHODS {
        let reply = client.rpc(method, json!({})).await;
        if row == "resources" || row == "prompts" {
            assert!(reply.get("result").is_some(), "{method}: implemented row answers a result: {reply}");
            continue;
        }
        assert_eq!(reply["error"]["code"], -32601, "{method}: {reply}");
        assert_eq!(reply["error"]["data"]["capability"], row, "{method}: {reply}");
    }
}
