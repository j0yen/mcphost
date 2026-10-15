//! PRD-mcphost-elicitation-with-ask-fallback: a session-mode streamable-HTTP
//! fixture client. The production router is stateless (a client's answer to a
//! server request would be dropped), so the typed `elicitation/create` path is
//! exercised over `build_router_with_session_mode(.., true)`, the same router
//! with rmcp's session routing on: the client `initialize`s with its
//! capabilities, then answers the server's request on the call's own stream.
//! The `answer` closure plays python-mcp's `elicitation_callback`.

use std::net::SocketAddr;
use std::sync::Arc;

use mcphost::state::AppState;
use serde_json::{Value, json};

/// How the fixture client responds to an `elicitation/create` request.
#[derive(Clone)]
pub enum Reply {
    /// `{action: "accept", content}`.
    Accept(Value),
    Decline,
    /// Never answers (the request just sits).
    Silent,
}

/// Serves the MCP handler over rmcp's session-mode streamable-HTTP service
/// on an ephemeral port; returns its base url. This is `build_router`'s
/// service without its stateless config and without the
/// `issue_session_id` layer (which rewrites `Mcp-Session-Id` and so hides
/// rmcp's own session from the client).
pub async fn serve_session_mode(state: Arc<AppState>) -> String {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let config = StreamableHttpServerConfig::default().with_legacy_session_mode(true).disable_allowed_hosts();
    let service = StreamableHttpService::new(
        move || Ok(mcphost::handler::McpHostHandler::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://{addr}")
}

pub struct SessionClient {
    http: reqwest::Client,
    base: String,
    key: String,
    session: String,
    next_id: std::sync::atomic::AtomicU64,
    /// Every `elicitation/create` request this client received.
    pub received: std::sync::Mutex<Vec<Value>>,
}

const VERSION: &str = "2025-06-18";

impl SessionClient {
    /// `initialize`s a session advertising form elicitation iff `elicitation`.
    pub async fn open(base: &str, key: &str, elicitation: bool) -> Self {
        let http = reqwest::Client::new();
        let capabilities = if elicitation { json!({"elicitation": {"form": {}}}) } else { json!({}) };
        let resp = http
            .post(format!("{base}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {key}"))
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": VERSION,
                "capabilities": capabilities,
                "clientInfo": {"name": "melc-fixture", "version": "1"},
            }}))
            .send()
            .await
            .expect("initialize");
        let session = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .expect("session id")
            .to_string();
        let _ = resp.text().await;
        let me = Self {
            http,
            base: base.to_string(),
            key: key.to_string(),
            session,
            next_id: std::sync::atomic::AtomicU64::new(2),
            received: Default::default(),
        };
        me.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), None).await;
        me
    }

    async fn post(&self, body: &Value, name: Option<&str>) -> reqwest::Response {
        let mut req = self
            .http
            .post(format!("{}/mcp", self.base))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Mcp-Session-Id", &self.session)
            .header("MCP-Protocol-Version", VERSION)
            .json(body);
        if let Some(method) = body.get("method").and_then(Value::as_str) {
            req = req.header("Mcp-Method", method);
        }
        if let Some(name) = name {
            req = req.header("Mcp-Name", name);
        }
        req.send().await.expect("post")
    }

    /// `tools/call`, answering any `elicitation/create` per `reply`. Returns
    /// the call's JSON-RPC response (`result` or `error`).
    pub async fn call(&self, name: &str, arguments: Value, reply: &Reply) -> Value {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut resp = self
            .post(
                &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}}),
                Some(name),
            )
            .await;
        let mut buf = String::new();
        loop {
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(60), resp.chunk())
                .await
                .expect("call timed out")
                .expect("read stream");
            let Some(chunk) = chunk else { panic!("stream ended without a response: {buf}") };
            buf.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
            while let Some(end) = buf.find("\n\n") {
                let block: String = buf.drain(..end + 2).collect();
                let Some(data) = block.lines().find_map(|l| l.strip_prefix("data:")) else { continue };
                let Ok(msg) = serde_json::from_str::<Value>(data.trim()) else { continue };
                if msg.get("method").and_then(Value::as_str) == Some("elicitation/create") {
                    self.received.lock().unwrap().push(msg.clone());
                    let result = match reply {
                        Reply::Accept(content) => json!({"action": "accept", "content": content}),
                        Reply::Decline => json!({"action": "decline"}),
                        Reply::Silent => continue,
                    };
                    self.post(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": result}), None).await;
                } else if msg.get("id") == Some(&json!(id)) && (msg.get("result").is_some() || msg.get("error").is_some()) {
                    return msg;
                }
            }
        }
    }
}
