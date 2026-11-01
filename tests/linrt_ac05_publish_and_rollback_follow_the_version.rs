//! PRD-mcphost-lineage-runtime-edge-writers AC5 (P0) -- Given tool `t` (published as `tool_t`:
//! tool names need two characters) published at v1 with `reads: ["a"]` then at v2 with `reads:
//! ["b"]`, and a chain `c` with a step on `t`, When lineage is read after the v2 publish and again
//! after `host.tool_rollback` to v1, Then the upstream of `tool:t` is `table:b` only after the
//! publish and `table:a` only after the rollback, and the edge `tool:t -> chain:c` (evidence
//! `chain_publish`) is present at both reads.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};

async fn trace(client: &McpClient, id: &str) -> Value {
    extract_structured(&client.tools_call("host.lineage.trace", json!({"id": id})).await.expect("trace"))
}

fn ids(list: &Value) -> Vec<String> {
    let mut out: Vec<String> =
        list.as_array().expect("list").iter().map(|e| e["id"].as_str().expect("id").to_string()).collect();
    out.sort();
    out
}

async fn assert_reads_and_chain_edge(client: &McpClient, reads: &str, step: &str) {
    let traced = trace(client, "tool:tool_t").await;
    assert_eq!(ids(&traced["upstream"]), vec![format!("table:{reads}")], "{step}: upstream of tool:tool_t: {traced:?}");
    let chain_edge = traced["downstream"]
        .as_array()
        .expect("downstream")
        .iter()
        .find(|e| e["id"] == json!("chain:chain_c"))
        .unwrap_or_else(|| panic!("{step}: chain:chain_c downstream of tool:tool_t: {traced:?}"));
    assert_eq!(chain_edge["evidence"], json!("chain_publish"), "{step}: {chain_edge:?}");
}

#[tokio::test]
async fn publish_and_rollback_move_the_reads_and_keep_the_chain_edge() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Linrt AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});

    publish(&client, "tool_t", "echo", json!({"schema": schema, "reads": ["a"]})).await;
    publish(&client, "chain_c", "chain", json!({"steps": [{"tool": "tool_t", "args": {}}]})).await;
    assert_reads_and_chain_edge(&client, "a", "after v1").await;

    publish(&client, "tool_t", "echo", json!({"schema": schema, "reads": ["b"]})).await;
    assert_reads_and_chain_edge(&client, "b", "after the v2 publish").await;

    client.tools_call("host.tool_rollback", json!({"name": "tool_t", "version": 1})).await.expect("rollback to v1");
    assert_reads_and_chain_edge(&client, "a", "after the rollback to v1").await;
}
