//! PRD-mcphost-spec-output-paths AC5 — Given the same tool with path
//! `$.json.absent`, When called, Then `result.payload` has no `absent` key
//! and `host.tool_test` reports the missing field alongside the path that
//! was tried.
//!
//! `envelope.missing` itself stays the plain field-name list every existing
//! `tests/envelope_ac*.rs` assertion depends on (requirement 9); the tried
//! path is additive, under `envelope.missing_detail` -- see
//! `kinds::envelope_report`'s doc comment for the wire shape.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_path_that_never_resolves_is_reported_with_the_path_tried() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "json": {"bridge_status": "active"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "SpecPath AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/anything", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": {"absent": "$.json.absent"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gap", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.gap"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);
    assert!(
        structured["payload"].get("absent").is_none(),
        "result.payload must have no `absent` key: {structured}"
    );

    let test_result = client
        .tools_call("host.tool_test", json!({"name": "gap", "args": {}}))
        .await
        .expect("tool_test ok");
    let test_structured = extract_structured(&test_result);

    let missing = test_structured["envelope"]["missing"]
        .as_array()
        .expect("envelope.missing must be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(
        missing.contains(&"absent"),
        "envelope.missing must still name absent: {test_structured}"
    );

    let detail = test_structured["envelope"]["missing_detail"]
        .as_array()
        .expect("envelope.missing_detail must be an array")
        .iter()
        .find(|d| d["name"] == "absent")
        .unwrap_or_else(|| panic!("missing_detail must have an entry for absent: {test_structured}"));
    assert_eq!(detail["path"], "$.json.absent");
}
