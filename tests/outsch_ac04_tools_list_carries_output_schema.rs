//! PRD-mcphost-contract-output-schemas
//! AC4 (P0) -- Given `tools/list` from a keyed tenant, When read, Then every
//! `host.*`/`billing.*` tool carries `outputSchema` equal to the contract's
//! `output_schema`; a tenant-published tool carries none unless its spec
//! declared `outputs`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, publish, signup};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn committed_contract() -> BTreeMap<String, Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-tools.v1.json");
    let doc: Value = serde_json::from_slice(&std::fs::read(path).expect("contract")).expect("json");
    doc["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| (t["name"].as_str().expect("name").to_string(), t.clone()))
        .collect()
}

#[tokio::test]
async fn host_tools_carry_the_contracts_output_schema_and_tenant_tools_only_declared_outputs() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Outsch AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "plain", "echo", json!({"schema": {"type": "object"}})).await;
    publish(
        &client,
        "declared",
        "http",
        json!({
            "method": "GET",
            "url": "https://example.invalid/v1/status",
            "args_schema": {"type": "object"},
            "outputs": ["bridge_status"],
        }),
    )
    .await;

    let listing = client.tools_list().await.expect("tools/list");
    let tools = listing["tools"].as_array().expect("tools array");
    let contract = committed_contract();

    let mut host_seen = 0;
    let mut problems = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().expect("name");
        if name.starts_with("host.") || name.starts_with("host_") || name.starts_with("billing") {
            host_seen += 1;
            let entry = &contract[name];
            let canonical = entry.get("alias_of").and_then(Value::as_str).unwrap_or(name);
            let expected = &contract[canonical]["output_schema"];
            if expected.is_null() {
                problems.push(format!("{name}: contract has no output_schema for {canonical}"));
            } else if tool.get("outputSchema") != Some(expected) {
                problems.push(format!("{name}: outputSchema differs from the contract's {canonical} output_schema"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(host_seen >= 170, "only {host_seen} host/billing tools listed");

    let by_name = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("{n} not listed"));
    assert!(by_name(&format!("{ns}.plain")).get("outputSchema").is_none(), "undeclared tool must carry none");
    let declared = by_name(&format!("{ns}.declared"));
    assert!(
        declared["outputSchema"]["properties"]["payload"]["properties"]
            .get("bridge_status")
            .is_some(),
        "declared outputs must surface: {declared}"
    );
}
