//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-contract-output-schemas
//! AC2 (P0) -- Given `host_tools(kinds, true)`, When every canonical tool is
//! checked, Then each has a non-empty `output_schema` of `type: object` (or
//! `oneOf` of objects); a tool without one fails with its name.

use mcphost::handler::{canonical_control_plane_names, host_tool_descriptors};
use mcphost::kinds::KindRegistry;
use serde_json::Value;

fn is_object_schema(schema: &serde_json::Map<String, Value>) -> bool {
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        return true;
    }
    schema
        .get("oneOf")
        .and_then(Value::as_array)
        .is_some_and(|branches| {
            !branches.is_empty()
                && branches
                    .iter()
                    .all(|b| b.get("type").and_then(Value::as_str) == Some("object"))
        })
}

#[test]
fn every_canonical_host_tool_has_a_non_empty_object_output_schema() {
    let kinds = KindRegistry::with_builtin();
    let canonicals = canonical_control_plane_names(&kinds);
    assert!(!canonicals.is_empty());
    let tools = host_tool_descriptors(&kinds);
    let mut problems = Vec::new();
    for name in &canonicals {
        let tool = tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .unwrap_or_else(|| panic!("{name} missing from host_tools"));
        match &tool.output_schema {
            None => problems.push(format!("{name}: no output_schema")),
            Some(schema) if schema.is_empty() => problems.push(format!("{name}: empty output_schema")),
            Some(schema) if !is_object_schema(schema) => {
                problems.push(format!("{name}: output_schema is not type object / oneOf of objects"))
            }
            Some(_) => {}
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_tool_with_no_registered_schema_is_reported_by_name() {
    assert!(mcphost::output_schemas::output_schema_for("host.no_such_tool").is_none());
    assert!(!is_object_schema(&serde_json::Map::new()));
}
