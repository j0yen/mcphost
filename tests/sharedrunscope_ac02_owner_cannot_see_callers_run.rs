//! PRD-mcphost-shared-call-run-scope
//! AC2 — Given the same call as AC1 (B's synchronous call on O's shared
//! `lookup`), When O calls `host.runs.list`, Then no run for it returns;
//! When O calls `host.runs.get {id}` with B's run id, Then `not_found`
//! (this codebase's wire code: `run_not_found`).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn owner_never_sees_the_callers_sync_shared_run() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "lookup", "visibility": "public", "description": "lookup"}),
        )
        .await
        .expect("O shares lookup publicly");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_o}.lookup");
    client_b
        .tools_call("host.tool_call", json!({"name": qualified, "args": {"msg": "hi"}}))
        .await
        .expect("B calls O's shared tool synchronously");

    let b_runs = extract_structured(
        &client_b
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("B lists its own runs"),
    );
    let run_id = b_runs["runs"][0]["run_id"]
        .as_str()
        .expect("B has a run id")
        .to_string();

    let owner_runs = extract_structured(
        &client_o
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("O lists its own runs"),
    );
    let owner_list = owner_runs["runs"].as_array().expect("runs array");
    assert!(
        owner_list.is_empty(),
        "owner must see none of the caller's runs: {owner_runs}"
    );

    let owner_get_err = client_o
        .tools_call("host.runs.get", json!({"run_id": run_id}))
        .await
        .expect_err("the owner must never resolve the caller's own run id");
    assert_eq!(owner_get_err.error_code.as_deref(), Some("run_not_found"));
}
