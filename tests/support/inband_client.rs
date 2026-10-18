//! PRD-mcphost-elicitation-in-band-on-stateless-transport: a raw MCP client
//! for the in-band tests. It talks to `TestServer` (so to `build_router`),
//! `initialize`s with whatever capabilities it is told, optionally echoes the
//! `Mcp-Session-Id` (the Claude Agent SDK replay never does), reads a
//! `tools/call` reply as `application/json` or `text/event-stream`, and
//! answers every server→client request through a closure -- the shape of
//! python-mcp's `elicitation_callback` / `sampling_callback`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

pub const VERSION: &str = "2025-06-18";

/// What the client does with one server→client request: wait, then post this
/// JSON-RPC `result`; `None` never answers.
pub type Answer = Option<(Duration, Value)>;

/// One `tools/call` as the client saw it.
pub struct Outcome {
    /// The call's JSON-RPC response (`result` or `error`).
    pub response: Value,
    /// The reply's `Content-Type`.
    pub content_type: String,
    /// Every server→client request that arrived on the call's own stream.
    pub frames: Vec<Value>,
    /// HTTP status of each answer the client POSTed.
    pub answer_statuses: Vec<u16>,
    /// HTTP status of the call's own reply.
    pub status: u16,
    /// Lowercased names of the reply's headers, sorted.
    pub header_names: Vec<String>,
    /// Time from sending the call to its first SSE event (None for a JSON body).
    pub first_event_after: Option<Duration>,
    /// Time from sending the call to its response.
    pub finished_after: Duration,
}

pub struct RawClient {
    http: reqwest::Client,
    base: String,
    key: String,
    /// The session id the server minted at `initialize`.
    pub session: String,
    echo_session: bool,
    next_id: AtomicU64,
}

impl RawClient {
    /// `initialize`s advertising `capabilities`; `echo_session` false plays a
    /// client that never sends `Mcp-Session-Id` back.
    pub async fn open(base: &str, key: &str, capabilities: Value, echo_session: bool) -> Self {
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{base}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {key}"))
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": VERSION,
                "capabilities": capabilities,
                "clientInfo": {"name": "elib-fixture", "version": "1"},
            }}))
            .send()
            .await
            .expect("initialize");
        let session = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .expect("the host mints a session id on initialize")
            .to_string();
        let _ = resp.text().await;
        let me = Self {
            http,
            base: base.to_string(),
            key: key.to_string(),
            session,
            echo_session,
            next_id: AtomicU64::new(2),
        };
        me.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), None, None).await;
        me
    }

    /// POSTs `body` to `/mcp`; the session header is `session` when given,
    /// else this client's own when it echoes.
    pub async fn post(&self, body: &Value, name: Option<&str>, session: Option<&str>) -> reqwest::Response {
        let mut req = self
            .http
            .post(format!("{}/mcp", self.base))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {}", self.key))
            .header("MCP-Protocol-Version", VERSION)
            .json(body);
        if let Some(session) = session.or(self.echo_session.then_some(self.session.as_str())) {
            req = req.header("Mcp-Session-Id", session);
        }
        if let Some(method) = body.get("method").and_then(Value::as_str) {
            req = req.header("Mcp-Method", method);
        }
        if let Some(name) = name {
            req = req.header("Mcp-Name", name);
        }
        req.send().await.expect("post")
    }

    /// `tools/call` `name`, answering each server→client request with
    /// `answer(frame)`. Panics if the call never completes in 60 s.
    pub async fn call(&self, name: &str, arguments: Value, answer: &(dyn Fn(&Value) -> Answer + Sync)) -> Outcome {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let started = std::time::Instant::now();
        let mut resp = self
            .post(
                &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}}),
                Some(name),
                None,
            )
            .await;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let status = resp.status().as_u16();
        let mut header_names: Vec<String> = resp.headers().keys().map(|k| k.as_str().to_string()).collect();
        header_names.sort();
        let mut out = Outcome {
            response: Value::Null,
            content_type,
            frames: Vec::new(),
            answer_statuses: Vec::new(),
            status,
            header_names,
            first_event_after: None,
            finished_after: Duration::ZERO,
        };
        let mut buf = String::new();
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(60), resp.chunk())
                .await
                .expect("call timed out")
                .expect("read stream");
            let Some(chunk) = chunk else {
                // A call that never asks is one `application/json` body.
                match serde_json::from_str::<Value>(buf.trim()) {
                    Ok(msg) if msg.get("id") == Some(&json!(id)) => {
                        out.response = msg;
                        out.finished_after = started.elapsed();
                        return out;
                    }
                    _ => panic!("stream ended without a response: {buf}"),
                }
            };
            buf.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
            while let Some(end) = buf.find("\n\n") {
                let block: String = buf.drain(..end + 2).collect();
                let Some(data) = block.lines().find_map(|l| l.strip_prefix("data:")) else { continue };
                let Ok(msg) = serde_json::from_str::<Value>(data.trim()) else { continue };
                out.first_event_after.get_or_insert_with(|| started.elapsed());
                if msg.get("method").and_then(Value::as_str).is_some() && msg.get("id").is_some() {
                    out.frames.push(msg.clone());
                    if let Some((delay, result)) = answer(&msg) {
                        tokio::time::sleep(delay).await;
                        let status = self
                            .post(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": result}), None, None)
                            .await
                            .status()
                            .as_u16();
                        out.answer_statuses.push(status);
                    }
                } else if msg.get("id") == Some(&json!(id)) && (msg.get("result").is_some() || msg.get("error").is_some()) {
                    out.response = msg;
                    out.finished_after = started.elapsed();
                    return out;
                }
            }
        }
    }
}

/// `accept {content}` for an `elicitation/create` frame.
pub fn accept(content: Value) -> Answer {
    Some((Duration::ZERO, json!({"action": "accept", "content": content})))
}

/// The fixture text for a `sampling/createMessage` frame.
pub fn sampled(text: &str) -> Answer {
    Some((
        Duration::ZERO,
        json!({"role": "assistant", "content": {"type": "text", "text": text}, "model": "fixture-model", "stopReason": "endTurn"}),
    ))
}

/// Never answers.
pub fn silent(_frame: &Value) -> Answer {
    None
}
