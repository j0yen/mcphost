//! PRD-mcphost-publish-schema-from-registry
//! AC3 (P0) -- Given one deliberately unknown field per kind, When
//! published, Then the server's `'<f>' is not a <kind> spec field; known:
//! …` list equals that kind's branch `properties` byte-for-byte (sorted),
//! proving schema and validator are one set.

use crate::common;
use common::{TempDataDir, TestServer, five_kinds_registry, signup};
use serde_json::json;

const WRONG_FIELD: &str = "zzz_not_a_field";

#[tokio::test]
async fn server_known_list_equals_the_published_branch_properties_for_every_kind() {
    let envs_dir = TempDataDir::new();
    let registry = five_kinds_registry(&envs_dir.0);
    let names: Vec<&'static str> = registry.names();
    let server = TestServer::start_with_kinds(registry).await;
    let (_ns, key) = signup(&server.base_url, "Pubschema AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list");
    let publish = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish is listed");
    let branches = publish["inputSchema"]["allOf"].as_array().expect("allOf branches");

    assert_eq!(names.len(), 5);
    for name in names {
        let branch = branches
            .iter()
            .find(|b| b["if"]["properties"]["kind"]["const"] == json!(name))
            .unwrap_or_else(|| panic!("no branch for {name}"));
        // serde_json's map is key-sorted, so this is the branch's sorted
        // property list exactly as published.
        let properties: Vec<&str> = branch["then"]["properties"]["spec"]["properties"]
            .as_object()
            .expect("branch properties")
            .keys()
            .map(String::as_str)
            .collect();
        let expected_tail = format!("known: {}", properties.join(", "));

        let err = client
            .tools_call(
                "host.tool_publish",
                json!({"name": format!("pubschema_{name}"), "kind": name, "spec": {WRONG_FIELD: 1}}),
            )
            .await
            .expect_err("an unknown spec field must fail the publish");

        assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"), "{name}: {err:?}");
        let prefix = format!("'{WRONG_FIELD}' is not a {name} spec field; ");
        assert!(err.message.contains(&prefix), "{name}: message {:?}", err.message);
        assert!(
            err.message.ends_with(&expected_tail) || err.message.contains(&format!("{expected_tail}.")),
            "{name}: server list must equal branch properties.\n  message: {}\n  branch:  {expected_tail}",
            err.message
        );
        let data_known: Vec<&str> = err.data["known"]
            .as_array()
            .expect("data.known")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        assert_eq!(data_known, properties, "{name}: data.known vs branch properties");
    }
}
