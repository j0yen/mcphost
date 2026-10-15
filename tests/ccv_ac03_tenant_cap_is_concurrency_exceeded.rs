//! AC3 (PRD-mcphost-call-concurrency-visible) — Given a free tenant with 4
//! calls in flight, When a 5th arrives, Then the error is
//! `concurrency_exceeded` with `data.in_flight == 4`, `data.limit == 4`,
//! `data.retry_after_ms > 0`, `data.scope == "tenant"`, and the `calls` row
//! has `error_class == "concurrency"`.
//!
//! The 4 calls in flight are held directly on the python kind's real
//! admission semaphores (`hold_admission_for_test`); the 5th is a real
//! `tools/call` through the server, refused at the tenant gate before any
//! sandbox work, so no working sandbox is needed.

use crate::common;
use common::{TestServer, poll_until_ready, signup};
use mcphost::kinds::{KindRegistry, python::PythonKind};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn ccv_ac03_fifth_call_is_concurrency_exceeded() {
    let envs_dir = common::TempDataDir::new();
    let mut kinds = KindRegistry::with_builtin();
    let py = Arc::new(PythonKind::new(&envs_dir.0));
    kinds.register(py.clone());
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "CCV AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db ok")
        .expect("tenant exists");

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "quick", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.quick");
    // Let the environment settle (whatever the call returns, it is not the
    // refusal under test: nothing is held yet).
    let _ = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(20)).await;

    let _four_in_flight = py.hold_admission_for_test(tenant.id, 4, 4);
    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("the 5th concurrent call must be refused");

    assert_eq!(err.error_code.as_deref(), Some("concurrency_exceeded"), "{err:?}");
    assert_eq!(err.data["in_flight"], json!(4), "{err:?}");
    assert_eq!(err.data["limit"], json!(4), "{err:?}");
    assert!(err.data["retry_after_ms"].as_u64().unwrap_or(0) > 0, "{err:?}");
    assert_eq!(err.data["scope"], json!("tenant"), "{err:?}");
    assert_eq!(err.data["plan"], json!("free"), "{err:?}");
    assert!(
        err.message.contains("4 of 4 concurrent calls in flight on this tenant"),
        "{}",
        err.message
    );

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let class: String = conn
        .query_row(
            "SELECT error_class FROM calls WHERE tenant_id = ?1 ORDER BY id DESC LIMIT 1",
            [tenant.id],
            |r| r.get(0),
        )
        .expect("the refused call's row");
    assert_eq!(class, "concurrency");
}
