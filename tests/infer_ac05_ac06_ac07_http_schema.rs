//! PRD-mcphost-tool-infer AC5/AC6/AC7 (P0) — Given an `http` spec with no
//! `args_schema` and placeholders across `url`, `headers`, `query` and
//! `body`, When the schema is inferred, Then every placeholder is a
//! required property except a placeholder resolving to a tenant secret,
//! which is excluded entirely.

use crate::common;
use common::{TestServer, http_kind_registry, signup};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn placeholders_across_every_template_field_become_properties_except_secrets() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC5-7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "api_key", "value": "shh"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "method": "POST",
        "url": "https://api.example.com/v1/{{ city }}",
        "headers": {"Authorization": "Bearer {{ secret.api_key }}"},
        "query": {"units": "{{ units }}"},
        "body": {"customer": "{{ customer_name }}"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "wrapper", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish without args_schema must succeed via inference");

    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == format!("{ns}.wrapper"))
        .expect("published tool listed")
        .clone();
    let schema = &tool["inputSchema"];

    // AC7: every placeholder across all four template kinds is a property.
    let required: BTreeSet<String> = schema["required"]
        .as_array()
        .expect("required array present")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    assert_eq!(
        required,
        BTreeSet::from([
            "city".to_string(),
            "units".to_string(),
            "customer_name".to_string(),
        ]),
        "url/query/body placeholders must all be required properties"
    );
    // AC5 (city specifically, from `url`).
    assert!(schema["properties"]["city"].is_object());

    // AC6: the secret reference is not a property of the argument schema.
    assert!(
        schema["properties"].get("secret").is_none(),
        "a secret.* reference must never become an argument property"
    );
    assert!(!required.contains("secret"));
}
