//! AC4 (P0) -- PRD-mcphost-resources-for-tenant-data. Given a subscription on
//! `mcphost://table/orders`, When `host.table.append` commits 2 rows, Then
//! one `notifications/resources/updated {uri}` reaches the peer within
//! 500 ms; Given `unsubscribe`, Then a further append sends nothing.
//!
//! Push needs a transport that keeps the server-to-client channel open, which
//! production's stateless `/mcp` does not; the test serves the same
//! `McpHostHandler` over rmcp's session-mode streamable HTTP and holds the
//! session's standalone GET stream open, as a real session client does.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::McpHostHandler;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const URI: &str = "mcphost://table/orders";

/// Serve `state`'s handler with rmcp's own sessions on an ephemeral port.
pub async fn serve_sessions(server: &TestServer) -> String {
    let state = server.state.clone();
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    let service = StreamableHttpService::new(
        move || Ok(McpHostHandler::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = axum::Router::new().route_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}/mcp")
}

/// A session client: POSTs with the session id, plus the standalone GET
/// stream whose frames `next_frames` collects.
pub struct SessionClient {
    http: reqwest::Client,
    url: String,
    key: String,
    session: String,
    stream: reqwest::Response,
    next_id: u64,
}

impl SessionClient {
    pub async fn open(url: &str, key: &str) -> Self {
        let http = reqwest::Client::new();
        let init = http
            .post(url)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "mres", "version": "0"}}}))
            .send()
            .await
            .expect("initialize");
        assert!(init.status().is_success(), "initialize: {}", init.status());
        let session = init.headers().get("mcp-session-id").expect("session id").to_str().unwrap().to_string();
        let _ = init.text().await;
        let initialized = http
            .post(url)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &session)
            .header("mcp-protocol-version", "2025-06-18")
            .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .send()
            .await
            .expect("initialized");
        assert!(initialized.status().is_success(), "initialized: {}", initialized.status());
        let stream = http
            .get(url)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "text/event-stream")
            .header("mcp-session-id", &session)
            .header("mcp-protocol-version", "2025-06-18")
            .send()
            .await
            .expect("GET stream");
        assert!(stream.status().is_success(), "GET stream: {}", stream.status());
        Self { http, url: url.to_string(), key: key.to_string(), session, stream, next_id: 2 }
    }

    /// One request on the session; the (JSON or SSE) body text.
    pub async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let resp = self
            .http
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", "2025-06-18")
            .json(&json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}))
            .send()
            .await
            .expect("rpc");
        let text = resp.text().await.expect("body");
        let payload = text
            .lines()
            .find_map(|l| l.strip_prefix("data:").map(str::trim).filter(|d| d.starts_with('{')))
            .map(str::to_string)
            .unwrap_or(text);
        serde_json::from_str(&payload).unwrap_or_else(|e| panic!("{method} reply not JSON ({e}): {payload}"))
    }

    /// Every JSON frame the GET stream delivers within `window`.
    pub async fn frames_within(&mut self, window: Duration) -> Vec<Value> {
        let deadline = Instant::now() + window;
        let mut buf = String::new();
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match tokio::time::timeout(left, self.stream.chunk()).await {
                Ok(Ok(Some(bytes))) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                _ => break,
            }
        }
        buf.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .collect()
    }

    /// End the session (DELETE), closing its transport.
    pub async fn close(self) {
        drop(self.stream);
        let _ = self
            .http
            .delete(&self.url)
            .header("Authorization", format!("Bearer {}", self.key))
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", "2025-06-18")
            .send()
            .await;
    }
}

pub async fn tenant_with_orders(server: &TestServer, name: &str) -> (McpClient, String) {
    let (_ns, key) = signup(&server.base_url, name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"sku": "text"}}))
        .await
        .expect("create orders");
    (client, key)
}

fn updated(frames: &[Value]) -> Vec<&Value> {
    frames.iter().filter(|f| f["method"] == "notifications/resources/updated").collect()
}

#[tokio::test]
async fn mres_ac04_append_notifies_the_subscriber_once_and_unsubscribe_silences_it() {
    let server = TestServer::start().await;
    let (writer, key) = tenant_with_orders(&server, "mres ac04").await;
    let url = serve_sessions(&server).await;
    let mut peer = SessionClient::open(&url, &key).await;

    let reply = peer.rpc("resources/subscribe", json!({"uri": URI})).await;
    assert!(reply.get("result").is_some(), "subscribe: {reply}");

    writer
        .tools_call("host.table.append", json!({"table": "orders", "rows": [{"sku": "a"}, {"sku": "b"}]}))
        .await
        .expect("append");
    let frames = peer.frames_within(Duration::from_millis(500)).await;
    let hits = updated(&frames);
    assert_eq!(hits.len(), 1, "exactly one updated notification within 500 ms of the append: {frames:?}");
    assert_eq!(hits[0]["params"]["uri"], URI, "{frames:?}");

    let reply = peer.rpc("resources/unsubscribe", json!({"uri": URI})).await;
    assert!(reply.get("result").is_some(), "unsubscribe: {reply}");
    writer
        .tools_call("host.table.append", json!({"table": "orders", "rows": [{"sku": "c"}]}))
        .await
        .expect("second append");
    let frames = peer.frames_within(Duration::from_millis(400)).await;
    assert!(updated(&frames).is_empty(), "no notification after unsubscribe: {frames:?}");
    peer.close().await;
}
