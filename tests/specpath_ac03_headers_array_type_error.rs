//! PRD-mcphost-spec-output-paths AC3 — Given an http spec with
//! `headers: ["X-A: 1"]`, When published, Then the error names `field` =
//! `headers`, `got` = `array`, `expected` = an object of header names to
//! values.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn headers_sent_as_a_list_names_the_field_and_expected_shape() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "SpecPath AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
        "headers": ["X-A: 1"],
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "badheaders", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("headers sent as a list (not a map) must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["field"], "headers");
    assert_eq!(err.data["got"], "array");
    assert!(
        err.data["expected"]
            .as_str()
            .is_some_and(|e| e.contains("object") && e.contains("header")),
        "expected must name an object of header names to values: {}",
        err.data
    );
    assert!(
        err.message.starts_with("invalid spec: headers:"),
        "message must start with 'invalid spec: headers:': {}",
        err.message
    );
}
