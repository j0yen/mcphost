//! AC8 — Given a template using an undefined variable, When called, Then
//! the error is `template_error` naming the variable.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn undefined_template_variable_names_itself() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Template Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        // `nope` is never in `args_schema` and never supplied as an
        // argument -- an unknown variable, not just a missing one.
        "url": "https://api.example.com/v1/things/{{nope}}",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "undefined_var", "kind": "http", "spec": spec}),
        )
        .await
        .expect(
            "publish ok (the host portion is a normal public host; only the path is templated)",
        );

    let err = client
        .tools_call(&format!("{ns}.undefined_var"), json!({}))
        .await
        .expect_err("an undefined template variable must fail the call");
    assert_eq!(err.error_code.as_deref(), Some("template_error"));
    assert_eq!(err.data["variable"], "nope");
}
