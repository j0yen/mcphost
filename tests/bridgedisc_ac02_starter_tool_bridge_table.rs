//! PRD-mcphost-sandbox-bridge-discoverability
//! AC2 (P0) -- Given the python `starter_tool` from quickstart published
//! verbatim, When `host.tool_test` runs its `test_call`, Then the result
//! contains a bridge `appended` count and `host.table.query` on the
//! starter's table returns the row.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn starter_tool_test_call_appends_and_the_row_is_queryable() {
    // This test publishes and runs a real sandboxed python tool, which
    // needs unprivileged user namespaces -- not guaranteed on GitHub's
    // hosted runners. Same skip convention every other sandbox-dependent
    // test in this crate uses.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "python"}))
            .await
            .expect("quickstart kind=python"),
    );
    let starter = &quickstart["starter_tool"];
    assert_eq!(starter["kind"], json!("python"));

    // Publish and test_call verbatim -- the exact two calls quickstart
    // handed back, no extra edits.
    let publish_call = &starter["publish_call"];
    client
        .tools_call(
            publish_call["call"].as_str().expect("publish_call.call"),
            publish_call["arguments"].clone(),
        )
        .await
        .expect("starter_tool.publish_call must publish on the free plan");

    let test_call = &starter["test_call"];
    let test_structured = extract_structured(
        &client
            .tools_call(
                test_call["call"].as_str().expect("test_call.call"),
                test_call["arguments"].clone(),
            )
            .await
            .expect("starter_tool.test_call must succeed"),
    );

    let appended = test_structured["result"]["appended"]
        .as_u64()
        .unwrap_or_else(|| panic!("result.appended must be a bridge count: {test_structured}"));
    assert_eq!(appended, 1, "appending one note must report appended: 1: {test_structured}");
    let table = test_structured["result"]["table"]
        .as_str()
        .unwrap_or_else(|| panic!("result.table must name the table the starter wrote to: {test_structured}"));

    // host.table.query on that exact table returns the row the starter's
    // own test_call just appended.
    let queried = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": format!("SELECT note FROM {table}")}))
            .await
            .expect("host.table.query on the starter's own table"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "the starter's test_call must have written exactly one row: {rows:?}");
    assert_eq!(rows[0]["note"], json!("hello"), "the row must carry the test_call's own input: {rows:?}");
}
