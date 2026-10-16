//! PRD-mcphost-idempotency-key-on-call AC1 (falsifier) — Given 20 fake-mode persona sessions whose planted tool
//! answers `tool_timeout` with the R6 hint, When each persona retries, Then
//! >= 10 retries carry the same `idempotency_key`.
//!
//! A fake persona follows the one rule the hint teaches: when the timeout
//! error names the key it sent, retry with that key; otherwise retry bare.
//! The planted tool times out on the first call and succeeds on the retry.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::idempotency::{ARG_KEY, TIMEOUT_HINT};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static PLANTED_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Times out on its first call per `persona` arg, answers `ok` afterwards.
struct PlantedTimeoutKind {
    seen: std::sync::Mutex<std::collections::HashSet<String>>,
}

#[async_trait]
impl Kind for PlantedTimeoutKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "idkey_planted_timeout"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "planted".to_string(),
            description: "times out once per persona".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        PLANTED_CALLS.fetch_add(1, Ordering::SeqCst);
        let persona = args["persona"].as_str().unwrap_or_default().to_string();
        let first = self.seen.lock().unwrap().insert(persona);
        if first {
            Err(KindError::structured("tool_timeout", "the call exceeded its timeout"))
        } else {
            Ok(json!({"ok": true}))
        }
    }
}

#[tokio::test]
async fn idkey_ac01() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PlantedTimeoutKind { seen: Default::default() }));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Idkey Ac01").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "planted", "kind": "idkey_planted_timeout", "spec": {}}),
        )
        .await
        .expect("publish planted tool");
    let tool = format!("{ns}.planted");

    let mut carried = 0;
    for i in 0..20 {
        let persona = format!("persona-{i}");
        let sent = format!("req-{i}");
        let client = McpClient::with_bearer(&server.base_url, &key);
        let err = client
            .tools_call(&tool, json!({"persona": persona, ARG_KEY: sent}))
            .await
            .expect_err("the planted tool times out first");
        assert_eq!(err.error_code.as_deref(), Some("tool_timeout"));
        assert_eq!(err.data["hint"], TIMEOUT_HINT, "R6 hint on the timeout error");
        // The persona's rule: retry with the key the error names, if any.
        let retry_key = err.data["idempotency_key"].as_str().map(str::to_string);
        let mut retry_args = json!({"persona": persona});
        if let Some(k) = &retry_key {
            retry_args[ARG_KEY] = json!(k);
        }
        client.tools_call(&tool, retry_args).await.expect("the retry succeeds");
        if retry_key.as_deref() == Some(sent.as_str()) {
            carried += 1;
        }
    }
    assert!(carried >= 10, "only {carried}/20 retries carried the same idempotency_key");
}
