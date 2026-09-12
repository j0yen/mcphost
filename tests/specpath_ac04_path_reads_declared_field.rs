//! PRD-mcphost-spec-output-paths AC4 — Given a published tool with
//! `outputs: {"bridge_status": "$.json.bridge_status"}` and an upstream that
//! returns `{"json": {"bridge_status": "active"}, "args": {}}`, When called,
//! Then `result.payload.bridge_status` is `"active"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
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
