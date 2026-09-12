//! PRD-mcphost-surface-fluidity AC1 — Given `host.quickstart(kind="python")`
//! for an authenticated tenant, When read, Then it contains
//! `try_before_call` with four rows naming `host.tool_test`,
//! `host.bridge_test`, `host.spec_test`, `host.tool_run` and an example call
//! each.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_try_before_call_names_all_four_dry_runs_with_an_example_call() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Surface AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&result);

    let rows = structured["try_before_call"]
        .as_array()
        .expect("try_before_call must be an array");
    let calls: Vec<&str> = rows.iter().filter_map(|r| r["call"].as_str()).collect();

    for expected in [
        "host.tool_test",
        "host.bridge_test",
        "host.spec_test",
        "host.tool_run",
    ] {
        assert!(
            calls.contains(&expected),
            "try_before_call must name {expected}: {rows:?}"
        );
    }
    assert_eq!(
        calls.len(),
        4,
        "try_before_call must have exactly four rows: {rows:?}"
    );
    // Every row carries a real, callable example -- not just the bare name.
    for row in rows {
        assert!(
            row["arguments"].is_object(),
            "row for {:?} must carry example arguments: {row}",
            row["call"]
        );
        assert!(
            row["case"].as_str().is_some_and(|s| !s.is_empty()),
            "row for {:?} must name its case: {row}",
            row["call"]
        );
    }
}
