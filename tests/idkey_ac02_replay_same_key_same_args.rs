//! PRD-mcphost-idempotency-key-on-call AC2 — Given a keyed call that succeeded, When the same key and args
//! arrive 1 s later, Then the response equals the first byte-for-byte except
//! `_meta.replayed: true` and the kind's call counter is unchanged.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::idempotency::ARG_KEY;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static RUNS: AtomicUsize = AtomicUsize::new(0);

/// Answers a different number on every execution, so a replayed response
/// can only match the first if the tool did not run again.
struct CounterKind;

#[async_trait]
impl Kind for CounterKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "idkey_counter"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "counter".to_string(),
            description: "counts executions".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        let n = RUNS.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(json!({"run": n, "args": args}))
    }
}

/// Drops the two replay markers so the rest can be compared byte for byte.
fn without_replay_markers(mut resp: Value) -> Value {
    if let Some(meta) = resp.get_mut("_meta").and_then(Value::as_object_mut) {
        meta.remove("replayed");
        meta.remove("first_seen_unix");
        if meta.is_empty() {
            resp.as_object_mut().unwrap().remove("_meta");
        }
    }
    resp
}

#[tokio::test]
async fn idkey_ac02() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(CounterKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Idkey Ac02").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "counter", "kind": "idkey_counter", "spec": {}}))
        .await
        .expect("publish");
    let tool = format!("{ns}.counter");

    let first = client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-8812"})).await.expect("first");
    assert!(first.pointer("/_meta/replayed").is_none(), "the first response is not a replay");
    assert_eq!(RUNS.load(Ordering::SeqCst), 1);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let again = client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-8812"})).await.expect("replay");

    assert_eq!(again.pointer("/_meta/replayed"), Some(&json!(true)));
    assert_eq!(RUNS.load(Ordering::SeqCst), 1, "the kind's call counter is unchanged");
    assert_eq!(
        serde_json::to_string(&without_replay_markers(first)).unwrap(),
        serde_json::to_string(&without_replay_markers(again)).unwrap(),
        "replay equals the first response except _meta.replayed"
    );

    // A different key is a different request: it runs.
    client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-8813"})).await.expect("new key");
    assert_eq!(RUNS.load(Ordering::SeqCst), 2);
}
