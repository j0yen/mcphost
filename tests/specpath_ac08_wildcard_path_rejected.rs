//! PRD-mcphost-spec-output-paths AC8 — Given `outputs: {"a": "$.items[*].a"}`,
//! When published, Then the error names `outputs.a` and says wildcards are
//! unsupported.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn a_wildcard_path_is_rejected_naming_the_field() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "SpecPath AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
        "outputs": {"a": "$.items[*].a"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "wildcard", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("a wildcard path must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["field"], "outputs.a");
    assert!(
        err.message.contains("wildcard"),
        "message must say wildcards are unsupported: {}",
        err.message
    );
    assert!(
        err.message.starts_with("invalid spec: outputs.a:"),
        "message must start with 'invalid spec: outputs.a:': {}",
        err.message
    );
}
