//! PRD-mcphost-first-publish-real-kind
//! AC8 (P1) -- Given `host.quickstart {kind: "http"}`, When executed
//! verbatim against the test host with the fixture upstream, Then the http
//! tool publishes and returns the fixture's JSON.

use crate::common;
use common::{TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};
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

    // PRD-mcphost-dry-run-side-effects requirement 4: `host.tool_test`
    // (the starter's `test_call`) now short-circuits the http call instead
    // of really hitting the upstream, so it no longer carries the
    // fixture's JSON -- that's proven below via the starter's own `next`
    // step, the real call.
    assert_eq!(test_structured["response"]["body"], Value::Null, "{test_structured}");
    assert_eq!(test_structured["dry_run_short_circuited"], json!(true));

    let starter_name = starter["name"].as_str().expect("starter.name");
    let real_result = client
        .tools_call(
            "host.tool_call",
            json!({"name": starter_name, "args": {}}),
        )
        .await
        .expect("the published starter tool's real call must succeed");
    let real_structured = extract_structured(&real_result);

    unsafe {
        std::env::remove_var("MCPHOST_HTTP_STARTER_URL");
    }

    assert_eq!(
        real_structured["body"]["fact"],
        json!("A group of cats is called a clowder."),
        "the real call must return the fixture's own JSON: {real_structured}"
    );
    assert_eq!(real_structured["body"]["length"], json!(38));
}
