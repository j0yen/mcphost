//! PRD-mcphost-idempotency-key-on-call-b AC1 — Given `_meta` stripped by the client, When `args.__idempotency_key` is sent, Then the tool's
//! `args` do not contain `__idempotency_key` and replay works.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::idempotency::ARG_KEY;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

static RUNS: AtomicUsize = AtomicUsize::new(0);
static SEEN: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Records the args it was handed and answers a fresh run number.
struct SpyKind;

#[async_trait]
impl Kind for SpyKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }
    fn name(&self) -> &'static str {
        "idkeyb_spy"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "spy".to_string(),
            description: "records its args".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        let n = RUNS.fetch_add(1, Ordering::SeqCst) + 1;
        SEEN.lock().unwrap().push(args);
        Ok(json!({"run": n}))
    }
}

async fn start() -> (TestServer, McpClient, String) {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(SpyKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Idkeyb").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "spy", "kind": "idkeyb_spy", "spec": {}}))
        .await
        .expect("publish");
    (server, client, format!("{ns}.spy"))
}

#[tokio::test]
async fn idkeyb_ac01() {
    let (_server, client, tool) = start().await;

    let first = client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-b1"})).await.expect("first");
    assert!(first.pointer("/_meta/replayed").is_none());
    let again = client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-b1"})).await.expect("replay");

    assert_eq!(again.pointer("/_meta/replayed"), Some(&json!(true)), "replay works with the key in args");
    assert_eq!(RUNS.load(Ordering::SeqCst), 1, "the tool ran once");
    let seen = SEEN.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].get(ARG_KEY).is_none(), "the tool's args must not contain {ARG_KEY}: {}", seen[0]);
    assert_eq!(seen[0].get("x"), Some(&json!(1)), "other args pass through untouched");
}
