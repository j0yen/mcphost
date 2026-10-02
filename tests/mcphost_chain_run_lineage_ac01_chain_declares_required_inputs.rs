//! PRD-mcphost-chain-run-lineage AC1 (P0) — Given a chain published with
//! steps `[{fetch_data, args:{url:"$.input.url"}}, {transform,
//! args:{rows:"$.prev.result.rows"}}, {write, args:{rows:"$.prev.result.rows",
//! region:"$.input.region"}}]`, When `host.tool_spec(name)` is read, Then
//! `input_schema.required == ["url","region"]` and `tools/list` shows the
//! same schema.
//!
//! There is no separate `host.tool_spec` RPC for an own-tenant tool (only
//! `host.tool_spec_shared`, for a cross-tenant read) -- an own tenant reads
//! its own tool's schema via `tools/list`, so this asserts both of AC1's
//! clauses against that one read.

use crate::common;
use common::{chain_kind_registry, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn chain_input_schema_required_derives_from_input_mapping_paths() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.rows"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(chain))
        .expect("published chain listed")
        .clone();

    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["url", "region"]),
        "tools/list inputSchema.required must name every $.input.* path, in \
         first-appearance order: {tool}"
    );
    assert!(
        tool["inputSchema"]["properties"]["url"].is_object(),
        "url must be a declared property: {tool}"
    );
    assert!(
        tool["inputSchema"]["properties"]["region"].is_object(),
        "region must be a declared property: {tool}"
    );
}
