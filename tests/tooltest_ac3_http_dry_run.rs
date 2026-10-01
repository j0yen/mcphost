//! PRD-mcphost-tool-test AC3 — Given an `http` spec with example args, When
//! tested, Then each invocation goes through the same outbound path and
//! restrictions as a published `http` tool would, dispatching once per
//! invocation.
//!
//! PRD-mcphost-dry-run-side-effects requirement 4 supersedes the original
//! "really hits the upstream" half of this AC for `http` specifically:
//! `host.spec_test` now short-circuits before `builder.send()` the same
//! way `host.tool_test` does, so the upstream sees zero requests and each
//! invocation's response is the short-circuited envelope (`status`/
//! `payload`: null) rather than a real 200. `host.bridge_test` remains the
//! tool for a real upstream probe.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn http_spec_short_circuits_every_invocation() {
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
        assert_eq!(inv["output"]["response"]["status"], Value::Null);
        assert_eq!(inv["output"]["dry_run_short_circuited"], json!(true));
    }

    let received = upstream.received_requests().await.expect("mock recorded");
    assert_eq!(
        received.len(),
        0,
        "host.spec_test must short-circuit every invocation, never hit the upstream"
    );
}
