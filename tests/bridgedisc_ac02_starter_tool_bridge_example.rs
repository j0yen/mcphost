//! PRD-mcphost-sandbox-bridge-discoverability
//! AC2 (P0) -- Given the python `starter_tool` from quickstart published
//! verbatim, When `host.tool_test` runs its `test_call`, Then the result
//! contains a bridge `appended` count and `host.table.query` on the
//! starter's table returns the row.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn starter_tool_persists_through_table_bridge() {
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
    let (_ns, key) = signup(&server.base_url, "Bridge AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "python"}))
            .await
            .expect("quickstart kind=python"),
    );
    let starter = &quickstart["starter_tool"];
    assert_eq!(starter["kind"], json!("python"), "starter_tool must be python: {starter}");

    // Execute publish_call, then test_call, verbatim -- exactly what
    // quickstart handed back, same convention as
    // tests/firstpub_ac01_quickstart_starter_tool.rs.
    let publish_call = &starter["publish_call"];
    client
        .tools_call(
            publish_call["call"].as_str().expect("publish_call.call"),
            publish_call["arguments"].clone(),
        )
        .await
        .expect("starter_tool.publish_call must publish on the free plan");

    let test_call = &starter["test_call"];
    let test_result = extract_structured(
        &client
            .tools_call(
                test_call["call"].as_str().expect("test_call.call"),
                test_call["arguments"].clone(),
            )
            .await
            .expect("starter_tool.test_call must succeed"),
    );

    let appended = test_result["result"]["appended"]
        .as_u64()
        .unwrap_or_else(|| panic!("test_call result must carry a bridge appended count: {test_result}"));
    assert_eq!(appended, 1, "the first call appends exactly one row: {test_result}");
    assert_eq!(
        test_result["result"]["words"],
        json!(1),
        "the starter's own words count must still be returned: {test_result}"
    );

    // AC2's second half: the sandboxed tool's own `mcphost.table.append`
    // call is visible to the ordinary session-side store, same proof
    // `tests/tables_ac05_python_sandbox_table_access.rs` already runs for
    // the tenant-tables PRD -- here it's the *starter* doing the
    // appending, not a test-authored tool.
    let table_name = test_result["result"]["table"]
        .as_str()
        .unwrap_or_else(|| panic!("bridge result must name the table: {test_result}"))
        .to_string();
    let queried = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": format!("SELECT text, words FROM {table_name}")}),
            )
            .await
            .expect("host.table.query on the starter's own table"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(
        rows.len(),
        1,
        "the row the starter appended must be visible to host.table.query: {queried}"
    );
    assert_eq!(rows[0]["text"], json!("hello"));
    assert_eq!(rows[0]["words"], json!(1));
}
