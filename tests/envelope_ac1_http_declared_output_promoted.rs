//! PRD-mcphost-result-envelope-contract AC1 — Given an http-kind tool whose
//! spec declares output field `bridge_status` and whose upstream returns
//! `{"data":{"bridge_status":"ok"}}`, When it is called, Then
//! `result.payload.bridge_status` equals `"ok"` and the original body
//! remains present.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn declared_field_nested_under_data_lands_at_result_payload() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"bridge_status": "ok"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Envelope AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/status", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": ["bridge_status"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "statustool", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.statustool"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["bridge_status"], "ok",
        "result.payload.bridge_status must equal \"ok\": {structured}"
    );
    // The original body remains present, at both its own location and
    // mirrored (with the promotion) into payload -- nothing is removed.
    assert_eq!(structured["body"]["data"]["bridge_status"], "ok");
    assert_eq!(structured["payload"]["data"]["bridge_status"], "ok");
}
