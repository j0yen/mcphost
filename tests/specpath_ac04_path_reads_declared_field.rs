//! PRD-mcphost-spec-output-paths AC4 — Given a published tool with
//! `outputs: {"bridge_status": "$.json.bridge_status"}` and an upstream that
//! returns `{"json": {"bridge_status": "active"}, "args": {}}`, When called,
//! Then `result.payload.bridge_status` is `"active"`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, http_kind_registry, publish, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn declared_path_reads_the_field_from_exactly_where_it_was_named() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "json": {"bridge_status": "active"},
            "args": {},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "SpecPath AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/anything", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": {"bridge_status": "$.json.bridge_status"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bridge", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.bridge"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["payload"]["bridge_status"], "active");
}

// Hotfix outputs-field-generated-from-kind-registry: `outputs` is a universal
// spec field (`kinds::UNIVERSAL_SPEC_FIELDS`), so a kind that used to refuse
// it (`'outputs' is not a echo spec field; known: reads, schema, ui`) now
// publishes it and exposes each declared name at `result.payload.<field>`.

#[tokio::test]
async fn echo_publishes_declared_outputs_and_exposes_them_at_result_payload() {
    let server = TestServer::start_with_kinds(mcphost::kinds::production_registry()).await;
    let (ns, key) = signup(&server.base_url, "Outputs Echo Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(
        &client,
        "say",
        "echo",
        json!({"schema": {"type": "object"}, "outputs": ["msg"]}),
    )
    .await;
    let result = client
        .tools_call(&format!("{ns}.say"), json!({"msg": "hi"}))
        .await
        .expect("call ok");
    assert_eq!(extract_structured(&result)["payload"]["msg"], "hi");
}

#[tokio::test]
async fn chain_publishes_declared_outputs_with_paths_and_bare_names() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Outputs Chain Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(
        &client,
        "pipe",
        "chain",
        json!({
            "steps": [{"tool": "src", "args": {"msg": "$.input.msg"}}],
            "outputs": {"greeting": "$.steps[0].result.msg", "echoed_input": "$.input.msg"},
        }),
    )
    .await;
    let result = client
        .tools_call(&format!("{ns}.pipe"), json!({"msg": "hi"}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);
    assert_eq!(structured["payload"]["greeting"], "hi", "{structured}");
    assert_eq!(structured["payload"]["echoed_input"], "hi", "{structured}");
}
