//! PRD-mcphost-rest-bridge AC1/AC2 — envelope conformance.
//!
//! AC1: given call shapes matching the baseline panel sessions
//! (`panel_integration_specialist_01`/`_03` -- a published `http`-kind
//! tool whose upstream returns JSON containing `bridge_status` and
//! `isolation_result`), calling the tool must place those fields at
//! `result.payload.<field>`, not just nested under `result.body`.
//!
//! AC2: an existing http-kind tool's response keeps its current shape --
//! `result.body` (and `result.status`/`result.headers`) unchanged -- with
//! `result.payload` added alongside, so no caller reading the old location
//! breaks.

mod common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn bridge_status_and_isolation_result_land_at_result_payload() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "bridge_status": "active",
            "isolation_result": "pass",
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Bridge Envelope Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/bridge", upstream.uri()),
        "args_schema": {"type": "object"},
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

    assert_eq!(
        structured["payload"]["bridge_status"], "active",
        "result.payload.bridge_status must be present: {structured}"
    );
    assert_eq!(
        structured["payload"]["isolation_result"], "pass",
        "result.payload.isolation_result must be present: {structured}"
    );
}

#[tokio::test]
async fn existing_body_location_is_unchanged_by_the_payload_addition() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Compat Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "legacy", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.legacy"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    // AC2: the existing locations are untouched...
    assert_eq!(structured["status"], 200);
    assert_eq!(structured["body"]["ok"], true);
    // ...and the new one carries exactly the same body content.
    assert_eq!(structured["payload"], structured["body"]);
}
