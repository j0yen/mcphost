//! AC4 — Given a published tool and arguments that violate `args_schema`,
//! When called, Then the call returns `args_invalid` with the schema path
//! and no upstream request is made.

mod common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn schema_invalid_args_are_rejected_before_any_request() {
    let upstream = MockServer::start().await;
    // If the host called out anyway, this would match and return 200 --
    // the test asserts on `upstream.received_requests()` below to prove it
    // never did.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Args Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/customers/{{{{id}}}}", upstream.uri()),
        "args_schema": {
            "type": "object",
            "properties": {"id": {"type": "string"}},
            "required": ["id"],
        },
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "needs_id", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Missing the required `id`.
    let err = client
        .tools_call(&format!("{ns}.needs_id"), json!({}))
        .await
        .expect_err("a schema-invalid call must fail");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));

    let requests = upstream
        .received_requests()
        .await
        .expect("mock server tracks requests");
    assert!(
        requests.is_empty(),
        "no upstream request may be made for schema-invalid arguments, got: {requests:?}"
    );
}
