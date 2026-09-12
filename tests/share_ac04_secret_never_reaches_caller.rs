//! AC4 — Given `geo` uses `{{secret:KEY}}`, When B calls it, Then the
//! upstream request carries A's secret and nothing B receives contains it
//! (the response is inspected, and B has no `host.tool_logs` entry for a
//! tool it never published locally). Modeled directly on
//! `http_ac01_secret_redaction.rs`, with the call coming from a second
//! tenant across a public share instead of the owner itself.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn cross_tenant_call_never_leaks_owners_secret_to_caller() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/customers/cus_123"))
        .and(header("Authorization", "Bearer sk_live_canary_secret_value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "cus_123",
            "echo_secret": "sk_live_canary_secret_value",
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_a
        .tools_call(
            "host.secret_set",
            json!({"name": "stripe", "value": "sk_live_canary_secret_value"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/customers/{{{{id}}}}", upstream.uri()),
        "headers": {"Authorization": "Bearer {{secret.stripe}}"},
        "args_schema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]},
    });
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "stripe_customer_get", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "stripe_customer_get", "visibility": "public"}),
        )
        .await
        .expect("share ok");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let result = client_b
        .tools_call(
            &format!("{ns_a}.stripe_customer_get"),
            json!({"id": "cus_123"}),
        )
        .await
        .expect("B calls A's shared tool");
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], 200);

    let dump = structured.to_string();
    assert!(
        !dump.contains("sk_live_canary_secret_value"),
        "secret value leaked into B's result: {dump}"
    );

    // B never published this tool locally, so B's own host.tool_logs has
    // nothing to say about it -- the secret cannot leak through a surface B
    // has no access to.
    let err = client_b
        .tools_call("host.tool_logs", json!({"name": "stripe_customer_get"}))
        .await
        .expect_err("B has no local tool by this name");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
}
