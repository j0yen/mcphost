//! PRD-mcphost-elicitation-with-ask-fallback: a streamable-HTTP fixture
//! client that `initialize`s with its capabilities, echoes `Mcp-Session-Id`,
//! then answers the server's `elicitation/create` on the call's own stream.
//! PRD-mcphost-elicitation-in-band-on-stateless-transport: it runs against
//! `build_router` itself (no test-only router); the `Reply` plays python-mcp's
//! `elicitation_callback`.

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

/// Serves the MCP handler on an ephemeral port through `build_router` -- the
/// production constructor, via `serve_on_listener` like `TestServer` -- and
/// returns its base url. PRD-mcphost-elicitation-in-band-on-stateless-transport
/// R9: this used to build a second, legacy-session-mode router; there is no
/// second router any more, so the client's `initialize` capabilities ride the
/// `Mcp-Session-Id` the host mints and the answer is routed by the host.
pub async fn serve_session_mode(state: Arc<AppState>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = mcphost::http::serve_on_listener(listener, state).await;
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
            let Some(chunk) = chunk else {
                // A call that never elicits is one `application/json` body, not a stream.
                match serde_json::from_str::<Value>(buf.trim()) {
                    Ok(msg) if msg.get("id") == Some(&json!(id)) => return msg,
                    _ => panic!("stream ended without a response: {buf}"),
                }
            };
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
