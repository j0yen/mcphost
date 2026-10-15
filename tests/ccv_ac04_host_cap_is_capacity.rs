//! AC4 (PRD-mcphost-call-concurrency-visible) — Given the host semaphore
//! exhausted by other tenants, When a tenant with 0 in flight calls, Then
//! the error is `capacity` with `data.scope == "host"`, `data.in_flight ==
//! 20`, `data.limit == 20` (and the `calls` row keeps `error_class ==
//! "capacity"`).
//!
//! "Other tenants" are five stand-in tenant ids each holding the free
//! plan's 4 slots (5 x 4 = 20 box-wide slots) on the python kind's real
//! admission semaphores (`hold_admission_for_test`); the caller's own
//! tenant gate is empty, so the refusal comes from the host gate.

use crate::common;
use common::{TestServer, poll_until_ready, signup};
use mcphost::kinds::{KindRegistry, python::PythonKind};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn ccv_ac04_host_exhausted_by_others_is_capacity_scope_host() {
    let envs_dir = common::TempDataDir::new();
    let mut kinds = KindRegistry::with_builtin();
    let py = Arc::new(PythonKind::new(&envs_dir.0));
    kinds.register(py.clone());
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "CCV AC4 Tenant").await;
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
    let _ = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(20)).await;

    let others: Vec<_> = (1..=5)
        .map(|i| py.hold_admission_for_test(tenant.id + 1000 * i, 4, 4))
        .collect();

    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("a call with the host at its ceiling must be refused");
    assert_eq!(err.error_code.as_deref(), Some("capacity"), "{err:?}");
    assert_eq!(err.data["scope"], json!("host"), "{err:?}");
    assert_eq!(err.data["in_flight"], json!(20), "{err:?}");
    assert_eq!(err.data["limit"], json!(20), "{err:?}");
    assert!(err.data["retry_after_ms"].as_u64().unwrap_or(0) > 0, "{err:?}");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let class: String = conn
        .query_row(
            "SELECT error_class FROM calls WHERE tenant_id = ?1 ORDER BY id DESC LIMIT 1",
            [tenant.id],
            |r| r.get(0),
        )
        .expect("the refused call's row");
    assert_eq!(class, "capacity");
    drop(others);
}
