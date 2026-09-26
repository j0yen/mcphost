//! PRD-mcphost-first-publish-real-kind
//! AC8 (P1) -- Given `host.quickstart {kind: "http"}`, When executed
//! verbatim against the test host with the fixture upstream, Then the http
//! tool publishes and returns the fixture's JSON.

use crate::common;
use common::{TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn quickstart_http_starter_publishes_and_returns_the_fixtures_json() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "fact": "A group of cats is called a clowder.",
            "length": 38,
        })))
        .mount(&upstream)
        .await;

    // SAFETY: this file has exactly one #[tokio::test] fn, so no sibling
    // test in this binary can observe (or race) this process-wide env var
    // -- same rationale `kinds::python`'s own
    // `ensure_uv_discoverable_for_test` doc comment gives for its `PATH`
    // mutation, and `mcphost_python_dependency_policy_ac03`'s own
    // `MCPHOST_ADVISORY_MODE` mutation.
    unsafe {
        std::env::set_var("MCPHOST_HTTP_STARTER_URL", format!("{}/fact", upstream.uri()));
    }

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "http"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&result);
    let starter = &structured["starter_tool"];
    assert_eq!(starter["kind"], json!("http"), "{structured}");

    let publish_call = &starter["publish_call"];
    let publish_result = client
        .tools_call(
            publish_call["call"].as_str().expect("publish_call.call"),
            publish_call["arguments"].clone(),
        )
        .await
        .expect("starter_tool.publish_call must publish verbatim");
    assert_eq!(extract_structured(&publish_result)["kind"], json!("http"));

    let test_call = &starter["test_call"];
    let test_result = client
        .tools_call(
            test_call["call"].as_str().expect("test_call.call"),
            test_call["arguments"].clone(),
        )
        .await
        .expect("starter_tool.test_call must succeed verbatim");
    let test_structured = extract_structured(&test_result);

    unsafe {
        std::env::remove_var("MCPHOST_HTTP_STARTER_URL");
    }

    assert_eq!(
        test_structured["response"]["body"]["fact"],
        json!("A group of cats is called a clowder."),
        "must return the fixture's own JSON: {test_structured}"
    );
    assert_eq!(test_structured["response"]["body"]["length"], json!(38));
}
