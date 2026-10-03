//! PRD-mcphost-sandbox-bridge-discoverability
//! AC2 (P0) -- Given the python `starter_tool` from quickstart published
//! verbatim, When `host.tool_test` runs its `test_call`, Then the result
//! contains a bridge `appended` count and the write is reported under
//! `dry_run.writes`, rolled back per PRD-mcphost-dry-run-side-effects --
//! `host.table.list` shows the starter's table was never persisted.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn starter_tool_test_call_appends_and_rolls_back() {
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
        .unwrap_or_else(|| panic!("result.table must name the table the starter wrote to: {test_structured}"))
        .to_string();

    assert_eq!(
        test_structured["dry_run"]["writes"],
        json!([{"store": "table", "op": "append", "table": table, "rows": 1}]),
        "{test_structured:?}"
    );
    assert_eq!(test_structured["dry_run"]["rolled_back"], json!(true), "{test_structured:?}");

    // host.tool_test is a dry run (PRD-mcphost-dry-run-side-effects): the
    // starter's own test_call sees its write inside the savepoint
    // (result.appended above), but nothing survives the call. The starter
    // creates its table lazily on first use, so this is the tenant's very
    // first test_call ever -- the create is rolled back along with the
    // append, and the table itself never exists outside the savepoint
    // (host.table.query on it would fail with table_not_found, not an
    // empty row set); host.table.list is the side-effect-free way to
    // confirm nothing persisted.
    let listed = extract_structured(
        &client
            .tools_call("host.table.list", json!({}))
            .await
            .expect("host.table.list"),
    );
    let tables = listed["tables"].as_array().expect("tables array");
    assert!(
        !tables.iter().any(|t| t["name"] == json!(table)),
        "a dry run must not persist the starter's table create+append: {tables:?}"
    );
}
