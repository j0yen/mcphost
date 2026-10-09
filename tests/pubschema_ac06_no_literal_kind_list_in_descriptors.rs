//! PRD-mcphost-publish-schema-from-registry
//! AC6 (P1) -- Given the descriptor strings and the MCP `instructions`
//! text, When grepped for the literal five-name kind sequence, Then there
//! is no hit; the list is rendered from the registry or replaced by a
//! reference to `enum`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, five_kinds_registry, signup};
use serde_json::Value;

/// Every way the five names read as one sequence in prose.
const FORBIDDEN: &[&str] = &[
    "chain, echo, http, python, wasm",
    "echo, http, python, chain, wasm",
    "echo, http, python, wasm, chain",
    "chain, echo, http, python and wasm",
    "echo, http, python, chain and wasm",
];

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

#[tokio::test]
async fn descriptor_strings_and_instructions_carry_no_literal_kind_sequence() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(five_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Pubschema AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let mut strings = Vec::new();
    let listed = client.tools_list().await.expect("tools/list");
    for tool in listed["tools"].as_array().expect("tools") {
        // Descriptions plus every string inside the input schema, except
        // the `enum` arrays, which are the one sanctioned rendering.
        strings.push(tool["description"].as_str().unwrap_or_default().to_string());
        let mut schema = tool["inputSchema"].clone();
        strip_enums(&mut schema);
        collect_strings(&schema, &mut strings);
    }
    let init = McpClient::new(&server.base_url).initialize().await;
    strings.push(init["result"]["instructions"].as_str().expect("instructions").to_string());
    assert!(strings.len() > 50, "collected too few descriptor strings: {}", strings.len());

    for text in &strings {
        let lower = text.to_lowercase();
        for needle in FORBIDDEN {
            assert!(!lower.contains(needle), "literal kind list {needle:?} found in: {text}");
        }
    }
}

fn strip_enums(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("enum");
            map.values_mut().for_each(strip_enums);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_enums),
        _ => {}
    }
}
