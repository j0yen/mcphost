//! PRD-mcphost-idempotency-key-on-call-b AC2 — Given a stored row older than 24 h, When the key is reused, Then the tool runs again and the
//! row is replaced.

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
async fn idkeyb_ac02() {
    let (server, client, tool) = start().await;

    let first = client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-b2"})).await.expect("first");
    assert!(first.to_string().contains("\"run\":1"), "first run: {first}");

    // Age the stored row past its 24 h TTL.
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let aged = 25 * 60 * 60;
    let n = conn
        .execute(
            "UPDATE call_results SET created_unix = created_unix - ?1, expires_unix = expires_unix - ?1 WHERE key = 'inv-b2'",
            [aged],
        )
        .expect("age row");
    assert_eq!(n, 1, "exactly one stored row");
    let old_created: i64 =
        conn.query_row("SELECT created_unix FROM call_results WHERE key = 'inv-b2'", [], |r| r.get(0)).unwrap();

    let again = client.tools_call(&tool, json!({"x": 1, ARG_KEY: "inv-b2"})).await.expect("rerun");
    assert!(again.pointer("/_meta/replayed").is_none(), "an expired row is not replayed");
    assert!(again.to_string().contains("\"run\":2"), "the tool ran again: {again}");
    assert_eq!(RUNS.load(Ordering::SeqCst), 2);

    let (rows, created, result): (i64, i64, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(created_unix), MAX(result_json) FROM call_results WHERE key = 'inv-b2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(rows, 1, "the row is replaced, not duplicated");
    assert!(created > old_created + aged - 60, "the new row carries a fresh timestamp");
    assert!(result.contains("\"run\":2"), "the new row holds the second result: {result}");
}
