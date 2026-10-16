//! PRD-mcphost-connection-provider-registry AC1 (P0) -- Given `providers.toml`
//! with `testprov` added under `test-support`, When the crate builds and the
//! five surfaces are read, Then `host.connect.providers`, the `provider` enum,
//! `docs/connections.md`, `www/llms.txt` and the callback route table all
//! contain `testprov` with no edit outside `providers.toml`.
//!
//! The suite links the lib with `test-support`, so the generated table has the
//! `testprov` row; the committed docs are rendered without it (a production
//! build never lists it), so the doc surfaces are read through the same
//! renderers `mcphost gen-docs` uses, over the full table.

use crate::common;
use common::{extract_structured, signup};
use mcphost::connect::{self, Provider};
use serde_json::{Value, json};

#[tokio::test]
async fn cprov_ac01() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CPROV1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Surface 1: host.connect.providers.
    let listed = extract_structured(&client.tools_call("host.connect.providers", json!({})).await.expect("providers"));
    let names: Vec<&str> = listed["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .map(|p| p["name"].as_str().expect("name"))
        .collect();
    assert!(names.contains(&"testprov"), "host.connect.providers lacks testprov: {names:?}");
    assert_eq!(
        names,
        Provider::ALL.iter().map(|p| p.name).collect::<Vec<_>>(),
        "one row per Provider::ALL, in order"
    );

    // Surface 2: the `provider` enum in the tools/list descriptor.
    let tools = client.tools_list().await.expect("tools/list");
    let tool = tools["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "host.connect.providers")
        .expect("host.connect.providers is listed");
    let en: Vec<&str> = tool["inputSchema"]["properties"]["provider"]["enum"]
        .as_array()
        .expect("provider enum")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(en.contains(&"testprov"), "provider enum lacks testprov: {en:?}");

    // Surface 3: docs/connections.md; surface 4: the llms.txt section.
    let all: Vec<Provider> = Provider::ALL.to_vec();
    let doc = connect::render_connections_doc(&all);
    assert!(connect::names_in_doc(&doc).contains(&"testprov".to_string()), "docs/connections.md lacks testprov");
    let llms = connect::splice_llms_section("intro\n", &connect::render_llms_section(&all));
    assert!(connect::names_in_llms_section(&llms).contains(&"testprov".to_string()), "llms.txt lacks testprov");

    // Surface 5: the callback route table.
    assert!(
        connect::callback_route_table().contains(&"/connect/callback/testprov".to_string()),
        "route table lacks testprov"
    );

    // The only edit is providers.toml: the table is generated, never hand-written.
    assert!(
        !std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/connect/providers_gen.rs").exists(),
        "a hand-written src/connect/providers_gen.rs would shadow the generated table"
    );
    let toml = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("providers.toml"))
        .expect("providers.toml");
    assert!(toml.contains("[testprov]") && toml.contains("test_only = true"), "testprov row is test-only in providers.toml");
}
