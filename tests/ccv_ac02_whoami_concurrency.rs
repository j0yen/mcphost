//! AC2 (PRD-mcphost-call-concurrency-visible) — Given a free tenant with 1
//! call in flight, When it calls `host.whoami`, Then `concurrency ==
//! {limit: 4, in_flight: 1, host_limit: 20, host_in_flight: >= 1}`.
//!
//! The "call in flight" is held directly on the python kind's real
//! admission semaphores (`hold_admission_for_test`): the property under
//! test is the live count `whoami` reads off them, which does not depend on
//! a sandbox spawn.

use crate::common;
use common::{TestServer, signup};
use mcphost::kinds::{KindRegistry, python::PythonKind};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn ccv_ac02_whoami_reports_live_concurrency() {
    let envs_dir = common::TempDataDir::new();
    let mut kinds = KindRegistry::with_builtin();
    let py = Arc::new(PythonKind::new(&envs_dir.0));
    kinds.register(py.clone());
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "CCV AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db ok")
        .expect("tenant exists");

    let idle = client.tools_call("host.whoami", json!({})).await.expect("whoami ok");
    let idle = common::extract_structured(&idle);
    assert_eq!(
        idle["concurrency"],
        json!({"limit": 4, "in_flight": 0, "host_limit": 20, "host_in_flight": 0}),
        "idle whoami: {idle}"
    );

    let _one_in_flight = py.hold_admission_for_test(tenant.id, 4, 1);
    let who = client.tools_call("host.whoami", json!({})).await.expect("whoami ok");
    let concurrency = common::extract_structured(&who)["concurrency"].clone();
    assert_eq!(concurrency["limit"], json!(4), "{concurrency}");
    assert_eq!(concurrency["in_flight"], json!(1), "{concurrency}");
    assert_eq!(concurrency["host_limit"], json!(20), "{concurrency}");
    assert!(concurrency["host_in_flight"].as_u64().unwrap_or(0) >= 1, "{concurrency}");
}
