//! PRD-mcphost-rest-bridge AC6 (P1) — Given a valid and an invalid bridge
//! spec, When `host.bridge_test` runs each, Then the valid one reports the
//! mock's response without creating a tool and the invalid one reports the
//! failure class.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_valid_spec_reports_the_mocks_response_and_publishes_nothing() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"bridge_status": "ok"})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Bridge Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    let result = client
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect("bridge_test ok");
    let structured = extract_structured(&result);
    assert_eq!(structured["response"]["status"], 200);
    assert_eq!(structured["response"]["payload"]["bridge_status"], "ok");

    // No tool was created -- an empty tool_list proves `bridge_test` never
    // published anything under this tenant.
    let listed = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("tool_list ok");
    let tools = extract_structured(&listed)["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        tools.is_empty(),
        "host.bridge_test must not create a published tool: {tools:?}"
    );
}

#[tokio::test]
async fn an_invalid_spec_reports_its_failure_class_without_calling_anything() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Bridge Test Invalid Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Missing both `method`/`url` and `upstream` -- invalid regardless of
    // the upstream server (there isn't one).
    let spec = json!({});
    let err = client
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("an invalid spec must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
}
