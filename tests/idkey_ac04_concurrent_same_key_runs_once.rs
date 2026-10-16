//! PRD-mcphost-idempotency-key-on-call AC4 — Given two concurrent calls with the same key, When both complete,
//! Then the tool ran exactly once and both responses carry the same result.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::idempotency::ARG_KEY;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static RUNS: AtomicUsize = AtomicUsize::new(0);

/// Slow enough that the second call arrives while the first is running.
struct SlowKind;

#[async_trait]
impl Kind for SlowKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "idkey_slow"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "slow".to_string(),
            description: "sleeps, then answers its run number".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        let n = RUNS.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::time::sleep(Duration::from_millis(600)).await;
        Ok(json!({"run": n}))
    }
}

#[tokio::test]
async fn idkey_ac04() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(SlowKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Idkey Ac04").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "slow", "kind": "idkey_slow", "spec": {}}))
        .await
        .expect("publish");
    let tool = format!("{ns}.slow");

    let a = McpClient::with_bearer(&server.base_url, &key);
    let b = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"x": 1, ARG_KEY: "same-key"});
    let (ra, rb) = crate::deadline_budget!(10_000, {
        tokio::join!(a.tools_call(&tool, args.clone()), b.tools_call(&tool, args.clone()))
    });
    let (ra, rb) = (ra.expect("first caller"), rb.expect("second caller"));

    assert_eq!(RUNS.load(Ordering::SeqCst), 1, "the tool ran exactly once");
    assert_eq!(ra.pointer("/structuredContent"), rb.pointer("/structuredContent"));
    assert_eq!(ra.pointer("/structuredContent/run"), Some(&json!(1)), "{ra}");
    let replays = [&ra, &rb].iter().filter(|r| r.pointer("/_meta/replayed") == Some(&json!(true))).count();
    assert_eq!(replays, 1, "exactly one response is the replay");
}
