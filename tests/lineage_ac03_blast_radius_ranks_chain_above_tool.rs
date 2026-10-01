//! PRD-mcphost-lineage-blast-radius AC3 (P0) -- Given a chain whose step tool reads `orders` and 40 finished
//! runs of that chain, When `host.lineage.blast_radius("table:orders",
//! "drop")` is called, Then the chain appears with severity `breaking`,
//! action from the effect table, and `uses` 40, ranked above a tool with 3
//! uses.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use std::time::{Duration, Instant};

async fn run_async_and_wait(client: &common::McpClient, name: &str) {
    let enqueued = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": name, "args": {}, "async": true}))
            .await
            .unwrap_or_else(|e| panic!("enqueue '{name}' failed: {} {}", e.code, e.message)),
    );
    let run_id = enqueued["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        match got["status"].as_str() {
            Some("done") => return,
            Some("error") | Some("timeout") => panic!("run {run_id} finished with status {got}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "run {run_id} never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn chain_with_40_runs_outranks_tool_with_3() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Lineage AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "orders_step", "echo", json!({"schema": schema, "reads": ["orders"]})).await;
    publish(&client, "orders_reader", "echo", json!({"schema": schema, "reads": ["orders"]})).await;
    publish(
        &client,
        "pipeline",
        "chain",
        json!({"steps": [{"tool": "orders_step", "args": {}}]}),
    )
    .await;

    for _ in 0..3 {
        run_async_and_wait(&client, "orders_reader").await;
    }
    for _ in 0..40 {
        run_async_and_wait(&client, "pipeline").await;
    }

    let report = extract_structured(
        &client
            .tools_call("host.lineage.blast_radius", json!({"id": "table:orders", "change_kind": "drop"}))
            .await
            .expect("blast_radius"),
    );
    let impacted = report["impacted"].as_array().expect("impacted array");

    let chain_idx = impacted
        .iter()
        .position(|n| n["id"] == json!("chain:pipeline"))
        .unwrap_or_else(|| panic!("chain:pipeline not in impacted: {impacted:?}"));
    let tool_idx = impacted
        .iter()
        .position(|n| n["id"] == json!("tool:orders_reader"))
        .unwrap_or_else(|| panic!("tool:orders_reader not in impacted: {impacted:?}"));

    let chain = &impacted[chain_idx];
    assert_eq!(chain["severity"], json!("breaking"), "chain: {chain:?}");
    assert_eq!(chain["uses"], json!(40), "chain: {chain:?}");
    assert!(
        chain["action"].as_str().is_some_and(|a| !a.is_empty()),
        "chain action must be non-empty: {chain:?}"
    );

    let tool = &impacted[tool_idx];
    assert_eq!(tool["severity"], json!("breaking"), "tool: {tool:?}");
    assert_eq!(tool["uses"], json!(3), "tool: {tool:?}");

    assert!(
        chain_idx < tool_idx,
        "chain (40 uses) must rank above tool (3 uses): {impacted:?}"
    );
}
