//! PRD-mcphost-publish-schema-from-registry
//! AC2 (P0) -- Given tools/list, When the `spec` schema is read, Then for
//! every registered kind there is a branch keyed on that kind whose
//! `properties` equal `known_spec_fields()` ∪ `UNIVERSAL_SPEC_FIELDS` with
//! `additionalProperties: false`, and `required` is exactly `[schema]`,
//! `[source]`, `[steps]`, `[component]` for echo, python, chain, wasm and
//! absent for http.

use crate::common;
use common::{TempDataDir, TestServer, five_kinds_registry, signup};
use mcphost::kinds::UNIVERSAL_SPEC_FIELDS;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The `then.properties.spec` schema of the `allOf` branch whose `if`
/// pins `kind` to `name`.
fn branch_spec<'a>(input_schema: &'a Value, name: &str) -> Option<&'a Value> {
    input_schema["allOf"]
        .as_array()
        .expect("inputSchema.allOf is an array of per-kind branches")
        .iter()
        .find(|b| b["if"]["properties"]["kind"]["const"] == json!(name))
        .map(|b| &b["then"]["properties"]["spec"])
}

#[tokio::test]
async fn every_registered_kind_has_a_branch_with_its_exact_spec_fields() {
    let envs_dir = TempDataDir::new();
    let registry = five_kinds_registry(&envs_dir.0);
    let kinds: Vec<_> = registry.all().cloned().collect();
    let server = TestServer::start_with_kinds(registry).await;
    let (_ns, key) = signup(&server.base_url, "Pubschema AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let publish = result["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish is listed");
    let input_schema = &publish["inputSchema"];

    assert_eq!(kinds.len(), 5, "the production registry has five kinds");
    assert_eq!(
        input_schema["allOf"].as_array().expect("allOf").len(),
        kinds.len(),
        "exactly one branch per registered kind (none for aliases or outcome words)"
    );

    for kind in &kinds {
        let name = kind.name();
        let spec = branch_spec(input_schema, name).unwrap_or_else(|| panic!("no branch for kind {name}"));

        let mut expected: BTreeSet<String> = kind.known_spec_fields().iter().map(|f| f.to_string()).collect();
        expected.extend(UNIVERSAL_SPEC_FIELDS.iter().map(|f| f.to_string()));
        let got: BTreeSet<String> = spec["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} branch has no properties"))
            .keys()
            .cloned()
            .collect();
        assert_eq!(got, expected, "{name} branch properties");
        assert_eq!(spec["additionalProperties"], json!(false), "{name} branch is closed");

        let required = match name {
            "echo" => Some(json!(["schema"])),
            "python" => Some(json!(["source"])),
            "chain" => Some(json!(["steps"])),
            "wasm" => Some(json!(["component"])),
            "http" => None,
            other => panic!("unexpected registered kind {other}"),
        };
        match required {
            Some(required) => assert_eq!(spec["required"], required, "{name} required"),
            None => assert!(spec.get("required").is_none(), "http has no required: {spec}"),
        }
    }
}
