//! PRD-mcphost-output-schema-tenant-values-opaque
//! AC5 (P1) -- Given a tenant tool whose spec declares an output field
//! without a type, When its `outputSchema` is built, Then that field is `{}`,
//! and a `tools/list` entry for it carries no `string` type on that field.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, publish, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn an_untyped_declared_output_field_is_the_empty_schema_in_tools_list() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Oschv AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(
        &client,
        "declared",
        "http",
        json!({
            "method": "GET",
            "url": "https://example.invalid/v1/status",
            "args_schema": {"type": "object"},
            "outputs": ["bridge_status", "rows"],
        }),
    )
    .await;

    let listing = client.tools_list().await.expect("tools/list");
    let name = format!("{ns}.declared");
    let tool = listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == name.as_str())
        .unwrap_or_else(|| panic!("{name} not listed"));
    let fields = &tool["outputSchema"]["properties"]["payload"]["properties"];
    for field in ["bridge_status", "rows"] {
        assert_eq!(fields[field], json!({}), "{field} must be the empty schema: {tool}");
    }
    assert!(!fields.to_string().contains("\"string\""), "no string type on a declared field: {fields}");

    // And the value a spec author's field carries -- an array -- validates.
    let validator = jsonschema::validator_for(&tool["outputSchema"]).expect("outputSchema compiles");
    let sample: Value = json!({"payload": {"bridge_status": [1, 2.5, true], "rows": [[1, 2]]}});
    assert!(validator.is_valid(&sample), "array field values must validate");
}
