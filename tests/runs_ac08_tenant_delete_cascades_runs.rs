//! AC8 (P0) — Given `admin.tenant_delete`, When a tenant with runs is
//! deleted, Then its `runs` rows and stored results are gone.
//!
//! `runs.tenant_id` and `tenant_state_kv.tenant_id` both carry `ON DELETE
//! CASCADE` to `tenants(id)` (migrations 0014 and 0011 respectively) --
//! this proves the cascade actually fires for a real done job's row plus
//! its stored result, reading the database directly since `host.runs.get`
//! itself is unreachable once the tenant's key is gone.

mod common;
use common::{ADMIN_KEY, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn tenant_delete_removes_its_runs_rows_and_stored_results() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "echoer", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");

    // Wait for the job to actually finish (and write its stored result)
    // before deleting the tenant -- the cascade must remove a REAL result,
    // not an absent one.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(run) = server
            .state
            .db
            .get_run(run_id.clone(), tenant.id)
            .await
            .expect("get_run")
        {
            if run.status == "done" {
                assert!(run.result_ref.is_some(), "done run must have a result_ref");
                break;
            }
        }
        assert!(Instant::now() < deadline, "echo job must finish within 5s");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let admin_client = common::McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin_client
        .tools_call("admin.tenant_delete", json!({"tenant": ns}))
        .await
        .expect("tenant_delete ok");

    let after = server
        .state
        .db
        .get_run(run_id.clone(), tenant.id)
        .await
        .expect("get_run after delete");
    assert!(after.is_none(), "runs row must be gone after tenant_delete: {after:?}");

    let stored = server
        .state
        .db
        .state_kv_get(tenant.id, format!("runs/{run_id}"))
        .await
        .expect("state_kv_get after delete");
    assert!(stored.is_none(), "stored result must be gone after tenant_delete");
}
