//! PRD-mcphost-spec-output-paths AC6 — Given a tool with list-form
//! `outputs: ["ingestion_status"]` and an upstream body
//! `{"json": {"ingestion_status": "ok"}}`, When `host.tool_test` runs, Then
//! the report lists `ingestion_status` as missing with `seen_at` containing
//! `$.json.ingestion_status`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_bare_name_entry_gets_a_seen_at_hint_for_where_it_actually_is() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "json": {"ingestion_status": "ok"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "SpecPath AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/anything", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": ["ingestion_status"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ingest", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let test_result = client
        .tools_call("host.tool_test", json!({"name": "ingest", "args": {}}))
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&test_result);

    let detail = structured["envelope"]["missing_detail"]
        .as_array()
        .expect("envelope.missing_detail must be an array")
        .iter()
        .find(|d| d["name"] == "ingestion_status")
        .unwrap_or_else(|| panic!("missing_detail must have an entry for ingestion_status: {structured}"));
    let seen_at = detail["seen_at"]
        .as_array()
        .expect("seen_at must be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(
        seen_at.contains(&"$.json.ingestion_status"),
        "seen_at must contain $.json.ingestion_status: {structured}"
    );
}
