//! PRD-mcphost-retry-policy-table-generated AC4 (R2) — Given `_meta.attempt:
//! 3` on a call that fails `upstream_error` with `max_attempts: 3`, When
//! rendered, Then `attempts_left: 0` and `retryable: false`.
//!
//! Driven over the wire: a planted kind fails `upstream_error`, and the call
//! carries `_meta.attempt` the way a client reports it.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use mcphost::retry_policy::policy_for;
use serde_json::{Value, json};
use std::sync::Arc;

struct PlantedUpstreamErrorKind;

#[async_trait]
impl Kind for PlantedUpstreamErrorKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "rpol_planted_upstream_error"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "planted".to_string(),
            description: "always answers upstream_error".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        Err(KindError::structured("upstream_error", "upstream failed on its own side"))
    }
}

async fn error_data(client: &McpClient, tool: &str, attempt: Option<u64>) -> Value {
    let mut meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "rpol", "version": "0.1.0"},
    });
    if let Some(n) = attempt {
        meta["attempt"] = json!(n);
    }
    let resp = client
        .post_raw(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": tool, "arguments": {}, "_meta": meta},
        }))
        .await;
    let body: Value = resp.json().await.expect("parse response");
    body["error"]["data"].clone()
}

#[tokio::test]
async fn rpol_ac04() {
    assert_eq!(policy_for("upstream_error").max_attempts, 3, "the AC names max_attempts: 3");
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PlantedUpstreamErrorKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Rpol Ac04").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "planted", "kind": "rpol_planted_upstream_error", "spec": {}}))
        .await
        .expect("publish planted tool");
    let tool = format!("{ns}.planted");

    let last = error_data(&client, &tool, Some(3)).await;
    assert_eq!(last["error_code"], json!("upstream_error"), "{last}");
    assert_eq!(last["retry"]["attempts_left"], json!(0), "{last}");
    assert_eq!(last["retry"]["retryable"], json!(false), "{last}");

    // The control: attempt 1 (and no `_meta.attempt` at all) is still retryable.
    for attempt in [Some(1), None] {
        let first = error_data(&client, &tool, attempt).await;
        assert_eq!(first["retry"]["attempts_left"], json!(2), "{first}");
        assert_eq!(first["retry"]["retryable"], json!(true), "{first}");
    }
}
