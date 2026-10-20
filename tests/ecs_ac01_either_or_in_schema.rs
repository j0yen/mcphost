//! PRD-mcphost-empty-call-refusal-stated-in-schema
//! contract: reads registry handler::host_tool_descriptors
//! AC1 (P0) -- Given `tools/list`, When `host.import` and `host.key.delegate`
//! are read, Then each carries its either-or in `anyOf`, `properties` flat and
//! `x-mcphost-validated: true`.

use mcphost::handler::host_tool_descriptors;
use mcphost::host_args::VALIDATED_KEY;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

#[test]
fn import_and_delegate_state_their_either_or_in_any_of() {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    for (name, expected) in [
        ("host.import", json!([{"required": ["run_id"]}, {"required": ["url"]}])),
        ("host.key.delegate", json!([{"required": ["scopes"]}, {"required": ["bind"]}])),
    ] {
        let tool = tools.iter().find(|t| t.name.as_ref() == name).unwrap_or_else(|| panic!("{name} not listed"));
        let schema = &tool.input_schema;
        assert_eq!(schema.get("anyOf"), Some(&expected), "{name} anyOf");
        assert_eq!(schema.get(VALIDATED_KEY), Some(&Value::Bool(true)), "{name} validated");
        let props = schema.get("properties").and_then(Value::as_object).expect("properties");
        for branch in expected.as_array().unwrap() {
            let arg = branch["required"][0].as_str().unwrap();
            assert!(props.contains_key(arg), "{name}: `{arg}` must be a flat top-level property");
        }
        assert!(schema.get("oneOf").is_none() && schema.get("allOf").is_none(), "{name}: schema stays flat");
    }
}
