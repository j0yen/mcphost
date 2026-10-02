//! PRD-mcphost-chain-run-lineage AC4 (P0) — Given the chain from AC-1 and
//! args `{url:"…", region:"eu"}`, When run via `host.tool_call` and
//! awaited, Then `host.runs.get(parent).children` has exactly 3 rows in
//! step order, each with `trigger == "composition"`, `parent_run_id ==
//! parent`, `status == "done"`, `tool` equal to the step's tool.

use crate::common;
use common::{chain_kind_registry, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn a_successful_run_writes_three_ordered_done_children() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
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

    let call = client
        .tools_call(&chain, json!({"url": "https://example.com/data", "region": "eu"}))
        .await
        .expect("a call with both inputs must succeed");
    let structured = common::extract_structured(&call);
    assert!(
        structured["failed_steps"].as_array().unwrap().is_empty(),
        "every step must succeed: {structured}"
    );

    // The parent's own run row: `call_published_tool` writes it AFTER the
    // call returns, under the SAME run id it pre-generated and threaded
    // into `CallCtx::parent_run_id` before dispatch -- `host.runs.list()`
    // (children excluded by default) must show exactly the one parent row.
    let runs = common::extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    let list = runs["runs"].as_array().expect("runs array");
    assert_eq!(list.len(), 1, "only the parent row by default: {runs}");
    let parent = &list[0];
    assert_eq!(parent["tool"], json!("daily_pipeline"));
    assert_eq!(parent["status"], json!("done"));
    let parent_id = parent["run_id"].as_str().expect("parent run_id").to_string();

    let got = common::extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": parent_id}))
            .await
            .expect("runs.get ok"),
    );
    let children = got["children"].as_array().expect("children array");
    assert_eq!(children.len(), 3, "exactly 3 children in step order: {got}");
    let tools: Vec<&str> = children.iter().map(|c| c["tool"].as_str().unwrap()).collect();
    assert_eq!(tools, vec!["fetch_data", "transform", "write"]);
    for child in children {
        assert_eq!(child["trigger"], json!("composition"));
        assert_eq!(child["parent_run_id"], json!(parent_id));
        assert_eq!(child["status"], json!("done"));
    }
}
