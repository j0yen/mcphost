//! PRD-mcphost-chain-run-lineage AC5 (P0) — Given that same run, When
//! `host.runs.list()` is called with no filters, Then only the parent row
//! appears; When called with `parent_run_id=parent`, Then exactly the 3
//! children appear; When called with `trigger="composition"`, Then the 3
//! children appear.

use crate::common;
use common::{chain_kind_registry, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn runs_list_default_excludes_children_but_parent_run_id_and_trigger_filters_show_them() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.url"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    client
        .tools_call(&chain, json!({"url": "https://example.com/data", "region": "eu"}))
        .await
        .expect("call ok");

    // No filters: only the parent row.
    let default_list = common::extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    let default_rows = default_list["runs"].as_array().expect("runs array");
    assert_eq!(default_rows.len(), 1, "children excluded by default: {default_list}");
    let parent_id = default_rows[0]["run_id"].as_str().expect("parent run_id").to_string();

    // parent_run_id=parent: exactly the 3 children.
    let by_parent = common::extract_structured(
        &client
            .tools_call("host.runs.list", json!({"parent_run_id": parent_id}))
            .await
            .expect("runs.list ok"),
    );
    let by_parent_rows = by_parent["runs"].as_array().expect("runs array");
    assert_eq!(by_parent_rows.len(), 3, "exactly 3 children by parent_run_id: {by_parent}");
    for row in by_parent_rows {
        assert_eq!(row["parent_run_id"], json!(parent_id));
        assert_eq!(row["trigger"], json!("composition"));
    }

    // trigger="composition": the same 3 children.
    let by_trigger = common::extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "composition"}))
            .await
            .expect("runs.list ok"),
    );
    let by_trigger_rows = by_trigger["runs"].as_array().expect("runs array");
    assert_eq!(by_trigger_rows.len(), 3, "exactly 3 children by trigger=composition: {by_trigger}");
}
