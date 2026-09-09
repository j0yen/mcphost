//! PRD-mcphost-spec-output-paths AC2 — Given an http spec with
//! `outputs: {"a": 5}`, When published, Then the error is structured
//! `invalid_spec` with `field` = `outputs.a`, `got` = `number`, `expected`
//! naming a path string, and `example` = `"$.json.a"`, and the message
//! starts `invalid spec: outputs.a:`.

mod common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn a_non_string_path_value_names_the_field_got_and_expected() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "SpecPath AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
        "outputs": {"a": 5},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "badpath", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("a numeric path value must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["field"], "outputs.a");
    assert_eq!(err.data["got"], "number");
    assert!(
        err.data["expected"]
            .as_str()
            .is_some_and(|e| e.contains("path string")),
        "expected must name a path string: {}",
        err.data
    );
    assert_eq!(err.data["example"], "$.json.a");
    assert!(
        err.message.starts_with("invalid spec: outputs.a:"),
        "message must start with 'invalid spec: outputs.a:': {}",
        err.message
    );
}
