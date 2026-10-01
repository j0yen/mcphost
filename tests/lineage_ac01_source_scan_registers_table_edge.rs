//! PRD-mcphost-lineage-blast-radius AC1 (P0) -- Given a published `python` tool whose source calls
//! `mcphost.table.query("SELECT * FROM orders")`, When lineage is read,
//! Then an edge `table:orders -> tool:<name>` exists with evidence
//! `source_scan`.

use crate::common;
use common::{TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn publishing_a_python_tool_registers_a_source_scan_edge() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Lineage AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "orders_reader",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return mcphost.table.query(\"SELECT * FROM orders\")\n",
                },
            }),
        )
        .await
        .expect("publish orders_reader");

    let traced = client
        .tools_call("host.lineage.trace", json!({"id": "table:orders"}))
        .await
        .expect("host.lineage.trace");
    let structured = extract_structured(&traced);

    let downstream = structured["downstream"].as_array().expect("downstream is an array");
    let edge = downstream
        .iter()
        .find(|e| e["id"] == json!("tool:orders_reader"))
        .unwrap_or_else(|| panic!("expected tool:orders_reader in downstream: {downstream:?}"));
    assert_eq!(edge["kind"], json!("tool"));
    assert_eq!(edge["evidence"], json!("source_scan"), "edge: {edge:?}");
}
