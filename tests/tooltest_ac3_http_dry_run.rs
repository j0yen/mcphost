//! PRD-mcphost-tool-test AC3 — Given an `http` spec with example args, When
//! tested, Then the wrapped endpoint is called once per invocation through
//! the same outbound path and restrictions as a published `http` tool, and
//! the response carries status code and a bounded body excerpt.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn http_spec_is_called_once_per_invocation() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"spec_test": "ok"})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Http Spec Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    let result = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "http", "spec": spec, "invocations": [{}, {}]}),
        )
        .await
        .expect("spec_test ok");
    let structured = extract_structured(&result);
    let invocations = structured["invocations"].as_array().expect("array");
    assert_eq!(invocations.len(), 2);
    for inv in invocations {
        assert_eq!(inv["ok"], json!(true));
        assert_eq!(inv["output"]["response"]["status"], json!(200));
        assert_eq!(
            inv["output"]["response"]["payload"]["spec_test"],
            json!("ok")
        );
    }

    let received = upstream.received_requests().await.expect("mock recorded");
    assert_eq!(
        received.len(),
        2,
        "each invocation must hit the upstream once"
    );
}
