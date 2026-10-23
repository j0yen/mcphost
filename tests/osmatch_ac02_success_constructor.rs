//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-success-result-matches-output-schema
//! AC2 (P0) -- Given `host.oauth.provider` on a tenant with no provider and
//! `host.enduser.whoami` with no end user, When the one success constructor
//! builds each result, Then `structuredContent` is `{"value": null}`,
//! `content[0].text` is its serialization, both validate against the
//! advertised `outputSchema`, a planted array result is wrapped the same way,
//! the constructor source names no tool, and the changelog carries the R2 line.

use std::path::Path;

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::success_result;
use serde_json::{Value, json};

fn validator_for(tool: &str) -> jsonschema::Validator {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let t = tools.iter().find(|t| t.name.as_ref() == tool).expect("tool served");
    let schema = Value::Object((**t.output_schema.as_ref().expect("outputSchema")).clone());
    jsonschema::validator_for(&schema).expect("schema compiles")
}

/// The wire form of a result, as `tools/call` serializes it.
fn wire(result: rmcp::model::CallToolResult) -> Value {
    serde_json::to_value(result).expect("result serializes")
}

#[test]
fn a_null_result_is_wrapped_and_validates_for_both_nothing_here_tools() {
    for tool in ["host.oauth.provider", "host.enduser.whoami"] {
        let result = wire(success_result(Value::Null));
        assert_eq!(result["structuredContent"], json!({"value": null}), "{tool}");
        let text = result["content"][0]["text"].as_str().expect("content[0].text");
        let from_text: Value = serde_json::from_str(text).expect("text is JSON");
        assert_eq!(from_text, result["structuredContent"], "{tool}: text is the serialization");
        let validator = validator_for(tool);
        for (what, row) in [("structuredContent", &result["structuredContent"]), ("text", &from_text)] {
            if let Some(e) = validator.iter_errors(row).next() {
                panic!("{tool} {what}: {}: {e}", e.instance_path);
            }
        }
    }
}

#[test]
fn a_planted_array_is_wrapped_the_same_way_and_an_object_is_untouched() {
    let array = wire(success_result(json!([1, 2])));
    assert_eq!(array["structuredContent"], json!({"value": [1, 2]}));
    let object = wire(success_result(json!({"a": 1})));
    assert_eq!(object["structuredContent"], json!({"a": 1}));
}

#[tokio::test]
async fn real_dispatch_answers_the_wrapped_null_and_it_validates() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Osmatch").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for tool in ["host.oauth.provider", "host.enduser.whoami"] {
        let result = client.tools_call(tool, json!({})).await.expect("call succeeds");
        assert_eq!(result["structuredContent"], json!({"value": null}), "{tool}: {result}");
        if let Some(e) = validator_for(tool).iter_errors(&result["structuredContent"]).next() {
            panic!("{tool}: {}: {e}", e.instance_path);
        }
    }
}

#[test]
fn the_constructor_source_names_no_tool_and_the_changelog_carries_the_r2_line() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("src/output_schemas.rs")).expect("output_schemas.rs");
    let start = src.find("pub fn success_result").expect("success_result");
    let body = &src[start..start + src[start..].find("\n}\n").expect("end of fn")];
    for tool in ["host.", "host_", "billing", "oauth", "enduser", "whoami"] {
        assert!(!body.contains(tool), "the constructor names {tool:?}: {body}");
    }
    let log = std::fs::read_to_string(root.join("CHANGELOG.md")).expect("CHANGELOG.md");
    let line = log
        .lines()
        .find(|l| l.contains("host.oauth.provider") && l.contains("host.enduser.whoami"))
        .expect("a changelog line names both tools");
    assert!(line.contains("{\"value\": null}") && line.contains(".value"), "{line}");
}
