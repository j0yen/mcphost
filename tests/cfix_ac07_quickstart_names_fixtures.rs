//! PRD-mcphost-chain-dry-run AC7 (P1) -- Given `host.quickstart kind=chain`,
//! When rendered, Then the text names `fixtures` and the schema test finds
//! every printed argument name in `host.tool_test`'s input schema.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, signup};
use serde_json::{Value, json};

fn input_schema_properties(tools: &Value, name: &str) -> Vec<String> {
    let tool = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(name))
        .unwrap_or_else(|| panic!("tools/list names {name}"));
    tool["inputSchema"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_chain_quickstart_names_fixtures_and_every_printed_argument_is_real() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = common::extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "chain"}))
            .await
            .expect("host.quickstart kind=chain"),
    );
    let text = quickstart.to_string();
    assert!(text.contains("fixtures"), "the chain quickstart must name fixtures: {quickstart}");

    let tools = client.tools_list().await.expect("tools/list");
    let section = &quickstart["fixtures"];
    let mut checked = 0;
    for key in ["export_call", "test_call"] {
        let call = &section[key];
        let verb = call["call"].as_str().expect("call verb");
        let schema_names = input_schema_properties(&tools, verb);
        for printed in call["arguments"].as_object().expect("arguments").keys() {
            assert!(
                schema_names.contains(printed),
                "{verb}: printed argument '{printed}' is not in its input schema {schema_names:?}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 5, "both calls print their arguments: {section}");
    assert!(
        section["test_call"]["arguments"].get("fixtures").is_some(),
        "the printed host.tool_test call carries a fixture: {section}"
    );
}
