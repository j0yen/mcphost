//! PRD-mcphost-sampling-from-tools: a session-mode streamable-HTTP fixture
//! client that plays a client's `sampling/createMessage` callback. Like
//! `support/elicit_session.rs` it `initialize`s with the capabilities it is
//! told to advertise, then answers the server's request on the call's own
//! stream; unlike it, the answer can be delayed (the 30 s wall) and carries
//! the usage report the client "billed" in `_meta.usage`.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

const VERSION: &str = "2025-06-18";

/// How the fixture client answers a `sampling/createMessage` request.
#[derive(Clone)]
pub struct Answer {
    pub text: String,
    /// `_meta.usage.{inputTokens, outputTokens}` the client reports, if any.
    pub usage: Option<(u64, u64)>,
    /// Wait this long before answering.
    pub delay: Duration,
}

impl Answer {
    pub fn text(text: &str) -> Self {
        Self { text: text.to_string(), usage: None, delay: Duration::ZERO }
    }
    pub fn with_usage(mut self, input: u64, output: u64) -> Self {
        self.usage = Some((input, output));
        self
    }
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
    fn result(&self) -> Value {
        let mut out = json!({
            "role": "assistant",
            "content": {"type": "text", "text": self.text},
            "model": "fixture-model",
            "stopReason": "endTurn",
        });
        if let Some((input, output)) = self.usage {
            out["_meta"] = json!({"usage": {"inputTokens": input, "outputTokens": output}});
        }
        out
    }
}

struct Inner {
    http: reqwest::Client,
    base: String,
    key: String,
    session: String,
    next_id: AtomicU64,
}

impl Inner {
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
}

pub struct SamplingSession {
    inner: Arc<Inner>,
    /// Every `sampling/createMessage` request this client received.
    pub received: Arc<std::sync::Mutex<Vec<Value>>>,
    /// How many of them it has answered (the "fixture client's counter").
    pub answered: Arc<AtomicUsize>,
}

impl SamplingSession {
    /// `initialize`s a session advertising `sampling` iff `sampling`.
    pub async fn open(base: &str, key: &str, sampling: bool) -> Self {
        let http = reqwest::Client::new();
        let capabilities = if sampling { json!({"sampling": {}}) } else { json!({}) };
        let resp = http
            .post(format!("{base}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {key}"))
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": VERSION,
                "capabilities": capabilities,
                "clientInfo": {"name": "msmp-fixture", "version": "1"},
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
        let inner = Arc::new(Inner {
            http,
            base: base.to_string(),
            key: key.to_string(),
            session,
            next_id: AtomicU64::new(2),
        });
        inner.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), None).await;
        Self { inner, received: Default::default(), answered: Default::default() }
    }

    /// `tools/call`, answering any `sampling/createMessage` per `answer`.
    /// Returns the call's JSON-RPC response (`result` or `error`).
    pub async fn call(&self, name: &str, arguments: Value, answer: &Answer) -> Value {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let mut resp = self
            .inner
            .post(
                &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}}),
                Some(name),
            )
            .await;
        let mut buf = String::new();
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(90), resp.chunk())
                .await
                .expect("call timed out")
                .expect("read stream");
            let Some(chunk) = chunk else { panic!("stream ended without a response: {buf}") };
            buf.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
            while let Some(end) = buf.find("\n\n") {
                let block: String = buf.drain(..end + 2).collect();
                let Some(data) = block.lines().find_map(|l| l.strip_prefix("data:")) else { continue };
                let Ok(msg) = serde_json::from_str::<Value>(data.trim()) else { continue };
                if msg.get("method").and_then(Value::as_str) == Some("sampling/createMessage") {
                    self.received.lock().unwrap().push(msg.clone());
                    let inner = self.inner.clone();
                    let answered = self.answered.clone();
                    let answer = answer.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(answer.delay).await;
                        answered.fetch_add(1, Ordering::SeqCst);
                        inner.post(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": answer.result()}), None).await;
                    });
                } else if msg.get("id") == Some(&json!(id)) && (msg.get("result").is_some() || msg.get("error").is_some()) {
                    return msg;
                }
            }
        }
    }
}
