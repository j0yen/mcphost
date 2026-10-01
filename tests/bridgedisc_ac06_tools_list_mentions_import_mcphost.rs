//! PRD-mcphost-sandbox-bridge-discoverability
//! AC6 (P1) -- Given `tools/list`, When fetched, Then the
//! `host.tool_publish` description and the python kind's field help each
//! contain `import mcphost` and `mcphost.table`.
//!
//! "The python kind's field help" is `kinds::python::python_field_hint`'s
//! `source` entry (the only per-spec-field help text this crate renders) --
//! surfaced to a caller through a structured `invalid_spec` error's
//! `data.expected` when `source` has the wrong type, so this test triggers
//! exactly that rather than reaching into a private function directly.

use crate::common;
use common::{TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn tool_publish_description_names_import_mcphost() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridge AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    let publish = tools
        .iter()
        .find(|t| t["name"] == json!("host.tool_publish"))
        .expect("host.tool_publish is listed");
    let description = publish["description"].as_str().expect("description is a string");

    assert!(description.contains("import mcphost"), "{description}");
    assert!(description.contains("mcphost.table"), "{description}");
    assert!(
        description.len() <= 600,
        "host.tool_publish's description must stay within surface_ac07's 600-char budget, got {}: {description}",
        description.len()
    );
}

#[tokio::test]
async fn python_source_field_help_names_import_mcphost_and_table() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridge AC6 Field Help Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // `source` with the wrong JSON type (a number, not a string) triggers
    // `spec_parse_error` -> `python_field_hint("source")` -> the
    // structured `invalid_spec` error whose `data.expected` is this PRD's
    // updated field-help text.
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_source_type", "kind": "python", "spec": {"source": 123}}),
        )
        .await
        .expect_err("a non-string source must be rejected before any sandbox work");
    let expected = err.data["expected"]
        .as_str()
        .unwrap_or_else(|| panic!("data.expected missing: {err:?}"));
    assert!(expected.contains("import mcphost"), "{expected}");
    assert!(expected.contains("mcphost.table"), "{expected}");
}
