//! PRD-mcphost-spec-output-paths AC1 — Given an http spec with
//! `outputs: {"a": "$.json.a"}` sent as a map, When `host.tool_publish`
//! runs, Then publish succeeds and the stored normalized outputs carry name
//! `a` with path `$.json.a`.
//!
//! `host.tool_publish`'s success response doesn't echo normalized `outputs`
//! back (requirement 8, P1, deferred -- see this PRD's `deferred_acs`); the
//! only externally observable proof that the map form was accepted *and*
//! parsed into the right name/path pair is exercising it: publish, then
//! call the tool against an upstream shaped so only a correctly-parsed
//! `$.json.a` path would find the value.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn map_form_outputs_publishes_and_the_declared_path_resolves() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "json": {"a": "the-value"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "SpecPath AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/echo", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": {"a": "$.json.a"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "mapform", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish must succeed for the map form of `outputs`");

    let result = client
        .tools_call(&format!("{ns}.mapform"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["a"], "the-value",
        "the map form's path $.json.a must have been parsed and read: {structured}"
    );
}
