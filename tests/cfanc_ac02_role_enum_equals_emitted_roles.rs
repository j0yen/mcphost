//! PRD-mcphost-chain-fanout-children-excludes-composition-child AC2 (P0) --
//! Given the `host.runs.get` output schema in
//! `contracts/host-output-schemas.v1.json`, When the registry is dumped and
//! the contract test runs, Then `children[].role` is an enum whose members
//! equal the roles `run_to_json` can emit, and a test fails if either side
//! changes alone.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use mcphost::output_schemas::output_schema_for;
use mcphost::runs::RUN_ROLES;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn enum_members(schema: &Value, what: &str) -> BTreeSet<String> {
    schema
        .pointer("/properties/children/items/properties/role/enum")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{what}: children[].role is not an enum: {schema}"))
        .iter()
        .map(|v| v.as_str().expect("enum member is a string").to_string())
        .collect()
}

#[test]
fn the_schema_enum_equals_the_declared_role_set() {
    let schema = Value::Object((*output_schema_for("host.runs.get").expect("registered")).clone());
    let declared: BTreeSet<String> = RUN_ROLES.iter().map(|r| r.to_string()).collect();
    assert_eq!(enum_members(&schema, "host.runs.get"), declared, "schema enum and RUN_ROLES drifted apart");
}

#[tokio::test]
async fn every_emitted_role_is_in_the_enum_and_every_member_is_emitted() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfanc AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    publish(&client, "source", "echo", json!({"schema": schema})).await;
    publish(&client, "worker", "echo", json!({"schema": schema})).await;
    let chain = publish(
        &client,
        "fan",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": [1, 2]}},
            {"tool": "worker", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({})).await.expect("fan-out call succeeds");
    let runs = common::extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let parent_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();
    let got = common::extract_structured(
        &client.tools_call("host.runs.get", json!({"run_id": parent_id})).await.expect("runs.get"),
    );

    let emitted: BTreeSet<String> = got["children"]
        .as_array()
        .expect("children")
        .iter()
        .map(|c| c["role"].as_str().expect("role is a string").to_string())
        .collect();
    let schema = Value::Object((*output_schema_for("host.runs.get").expect("registered")).clone());
    assert_eq!(emitted, enum_members(&schema, "host.runs.get"), "emitted roles vs schema enum: {got}");

    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    if let Err(e) = validator.validate(&got) {
        panic!("host.runs.get output rejected by its schema: {e}\n{got}");
    }
}
