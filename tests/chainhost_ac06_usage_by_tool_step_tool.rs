//! PRD-mcphost-chain-host-steps AC6 (P1) — Given a chain with one host
//! step, When called once, Then `host.usage by=tool` shows one call for
//! the chain and the calls row carries `step_tool: "host.table.append"`.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn one_host_step_meters_as_one_call_with_step_tool_recorded() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Chain Host Step Usage Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "runs", "columns": {"id": "integer"}}),
        )
        .await
        .expect("host.table.create must succeed");

    let chain_spec = json!({
        "steps": [
            {"tool": "host.table.append", "args": {"table": "runs", "rows": [{"id": 1}]}}
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    client
        .tools_call(&chain, json!({}))
        .await
        .expect("the one-host-step chain call must succeed");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");

    // "host.usage by=tool shows one call for the chain": the chain's own
    // tool name, not `host.table.append`, carries exactly one call -- the
    // host step dispatches through `dispatch_tenant_tool` directly
    // (`handler.rs`'s ~150-arm match), which writes no `calls` row of its
    // own.
    let usage = client
        .tools_call("host.usage", json!({"by": "tool", "window": "1h"}))
        .await
        .expect("host.usage by=tool must succeed");
    let usage = common::extract_structured(&usage);
    let rows = usage["rows"].as_array().expect("rows array");
    let pipeline_entry = rows
        .iter()
        .find(|r| r["key"] == json!("pipeline"))
        .unwrap_or_else(|| panic!("no 'pipeline' entry in usage breakdown: {rows:?}"));
    assert_eq!(
        pipeline_entry["calls"],
        json!(1),
        "the chain's own tool name must show exactly one call: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r["key"] == json!("host.table.append")),
        "the host step must not add its own usage-by-tool entry: {rows:?}"
    );

    let step_tool = server
        .state
        .db
        .last_call_step_tool_for_test(tenant.id, "pipeline".to_string())
        .await
        .expect("query last call's step_tool")
        .expect("a calls row for 'pipeline' must exist");
    assert_eq!(
        step_tool,
        Some("host.table.append".to_string()),
        "the chain's own calls row must carry step_tool"
    );
}
