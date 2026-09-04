//! AC15 — Given a `Kind::call` that never completes, When the 30s
//! deadline passes, Then the caller receives error `call_timeout`, the
//! future is dropped, and the server's task count returns to baseline.
//!
//! Uses a real 30s deadline would make this test glacial; `TestServer`
//! exposes `call_timeout` precisely so this AC can be proven against a
//! short deadline instead of the PRD's production value (which `main.rs`
//! wires to the real 30s constant).
//!
//! "The future is dropped" and "task count returns to baseline" are
//! proven behaviorally: the never-completing `Kind::call` future
//! increments a counter on entry and decrements it in a `Drop` guard: if
//! the future weren't actually dropped on timeout (e.g. leaked into a
//! detached task), the counter would stay elevated after the call
//! returns.

mod common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

struct DropGuard;
impl Drop for DropGuard {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A kind whose call never resolves on its own; only a timeout ends it.
struct HangKind;

#[async_trait]
impl Kind for HangKind {
    fn name(&self) -> &'static str {
        "hang"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "hang".to_string(),
            description: "never completes".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        let _guard = DropGuard;
        std::future::pending::<()>().await;
        unreachable!("std::future::pending never resolves");
    }
}

#[tokio::test]
async fn hung_call_times_out_and_drops_its_future() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HangKind));
    let server = TestServer::start_full(
        Some(common::ADMIN_KEY.to_string()),
        kinds,
        Duration::from_millis(200),
        None,
    )
    .await;

    let (tenant_ns, key) = signup(&server.base_url, "Timeout Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "stall", "kind": "hang", "spec": {}}),
        )
        .await
        .expect("publish the hang kind");
    let qualified = format!("{tenant_ns}.stall");

    let started = std::time::Instant::now();
    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("a call that never completes must time out");
    assert_eq!(err.error_code.as_deref(), Some("call_timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the call must return at the configured deadline, not hang forever"
    );

    // Give the dropped future's Drop guard a moment to run, then confirm
    // no in-flight call was leaked.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        IN_FLIGHT.load(Ordering::SeqCst),
        0,
        "the timed-out future must have been dropped, not left running"
    );
}
