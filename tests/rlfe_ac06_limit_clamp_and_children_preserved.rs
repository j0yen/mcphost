//! contract: host.runs.list is the checked descriptor (x-mcphost-validated) and its limit clamp and children read are unchanged
//! PRD-mcphost-runs-list-filter-enums-generated
//! AC6 (P0) -- Given the checked descriptor, When `host.runs.list` is called
//! with `limit: 500, include_result: true`, with `limit: 0`, and with
//! `parent_run_id` of a chain run, Then the first is clamped to 50 with
//! `_meta.mcphost.limit_clamped`, the second is `args_invalid`, and the third
//! returns the composition children -- unchanged from 16abd56c.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::db::TestRunRow;
use mcphost::host_args::VALIDATED_KEY;
use mcphost::runs::{RunStatus, RunTrigger};
use serde_json::json;

#[tokio::test]
async fn rlfe_ac06() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "RLFE AC6 Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The descriptor is the checked one.
    let listed = client.tools_list().await.expect("tools/list");
    let tool = listed["tools"].as_array().unwrap().iter().find(|t| t["name"] == json!("host.runs.list")).unwrap();
    assert_eq!(tool["inputSchema"][VALIDATED_KEY], json!(true), "host.runs.list must be the checked descriptor");

    let seed = |parent: Option<String>, trigger: RunTrigger| TestRunRow {
        tenant_id: tenant.id,
        tool_name: "t".into(),
        status: RunStatus::Done,
        trigger,
        parent_run_id: parent,
        progress_json: None,
    };
    let parent = server.state.db.insert_run_row_for_test(seed(None, RunTrigger::Call)).await.expect("parent");
    for _ in 0..59 {
        server.state.db.insert_run_row_for_test(seed(None, RunTrigger::Call)).await.expect("seed");
    }
    let mut children = Vec::new();
    for _ in 0..3 {
        let id = server
            .state
            .db
            .insert_run_row_for_test(seed(Some(parent.clone()), RunTrigger::Composition))
            .await
            .expect("child");
        children.push(id);
    }

    // limit above the max: clamped to 50 and reported.
    let clamped = client
        .tools_call("host.runs.list", json!({"limit": 500, "include_result": true}))
        .await
        .expect("a limit above the max is clamped, not refused");
    assert_eq!(extract_structured(&clamped)["runs"].as_array().unwrap().len(), 50, "{clamped:?}");
    assert_eq!(clamped["_meta"]["mcphost"]["limit_clamped"], json!({"asked": 500, "max": 50}), "{clamped:?}");

    // limit 0: args_invalid.
    let zero = client.tools_call("host.runs.list", json!({"limit": 0})).await.expect_err("limit 0 is refused");
    assert_eq!(zero.error_code.as_deref(), Some("args_invalid"), "{zero:?}");

    // parent_run_id of a chain run: its composition children.
    let page = client.tools_call("host.runs.list", json!({"parent_run_id": parent})).await.expect("children");
    let page = extract_structured(&page);
    let mut got: Vec<String> =
        page["runs"].as_array().unwrap().iter().map(|r| r["run_id"].as_str().unwrap().to_string()).collect();
    got.sort();
    children.sort();
    assert_eq!(got, children, "{page}");
    assert!(page["runs"].as_array().unwrap().iter().all(|r| r["trigger"] == json!("composition")), "{page}");
}
