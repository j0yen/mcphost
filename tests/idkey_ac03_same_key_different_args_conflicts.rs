//! PRD-mcphost-idempotency-key-on-call AC3 — Given a keyed call, When the same key arrives with different args,
//! Then the code is `idempotency_conflict` and `data` carries both hashes.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::idempotency::{ARG_KEY, CONFLICT_HINT, args_sha256};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static RUNS: AtomicUsize = AtomicUsize::new(0);

struct EchoCountKind;

#[async_trait]
impl Kind for EchoCountKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "idkey_echo_count"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "echo_count".to_string(),
            description: "echoes args, counts runs".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(args)
    }
}

#[tokio::test]
async fn idkey_ac03() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(EchoCountKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Idkey Ac03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "echo", "kind": "idkey_echo_count", "spec": {}}))
        .await
        .expect("publish");
    let tool = format!("{ns}.echo");

    client.tools_call(&tool, json!({"amount": 10, ARG_KEY: "k-1"})).await.expect("first");
    let err = client
        .tools_call(&tool, json!({"amount": 99, ARG_KEY: "k-1"}))
        .await
        .expect_err("same key, different args conflicts");

    assert_eq!(err.error_code.as_deref(), Some("idempotency_conflict"));
    assert_eq!(err.data["stored_args_sha256"], args_sha256(&json!({"amount": 10})));
    assert_eq!(err.data["given_args_sha256"], args_sha256(&json!({"amount": 99})));
    assert_ne!(err.data["stored_args_sha256"], err.data["given_args_sha256"]);
    assert_eq!(err.data["key"], "k-1");
    assert_eq!(err.data["hint"], CONFLICT_HINT);
    assert_eq!(RUNS.load(Ordering::SeqCst), 1, "the conflicting call never ran the tool");

    // Key order inside args does not change the hash: same request, replays.
    client.tools_call(&tool, json!({"b": 1, "a": 2, ARG_KEY: "k-2"})).await.expect("k-2");
    let replay = client.tools_call(&tool, json!({"a": 2, "b": 1, ARG_KEY: "k-2"})).await.expect("k-2 again");
    assert_eq!(replay.pointer("/_meta/replayed"), Some(&json!(true)));
}
