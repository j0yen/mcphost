//! PRD-mcphost-spec-output-paths AC6 — Given a tool with list-form
//! `outputs: ["ingestion_status"]` and an upstream body
//! `{"json": {"ingestion_status": "ok"}}`, When `host.tool_test` runs, Then
//! the report lists `ingestion_status` as missing with `seen_at` containing
//! `$.json.ingestion_status`.
//!
//! PRD-mcphost-dry-run-side-effects requirement 4 supersedes this AC's
//! original http-kind demonstration: `host.tool_test` now short-circuits
//! an http call before the upstream ever answers (the same change that
//! makes `host.bridge_test`/the real call the tools for a real response),
//! so there is no real body left to search for the `seen_at` hint in --
//! `http::Kind::source_for_output_search` sees `response.body: null` and
//! correctly reports nothing, rather than a stale or fabricated path.
//! `envelope`/`seen_at` is computed only inside `host.tool_test` (no other
//! tool calls `envelope_report`), so there is no other entry point left to
//! demonstrate a genuine `seen_at` hit for `http` through this host at
//! all; this test now proves the (correct, new) absence instead.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_bare_name_entry_gets_no_seen_at_hint_once_tool_test_short_circuits_http() {
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

    assert_eq!(structured["dry_run_short_circuited"], json!(true));
    let detail = structured["envelope"]["missing_detail"]
        .as_array()
        .expect("envelope.missing_detail must be an array")
        .iter()
        .find(|d| d["name"] == "ingestion_status")
        .unwrap_or_else(|| panic!("missing_detail must have an entry for ingestion_status: {structured}"));
    // The upstream was never called, so there is no real body left to find
    // a `seen_at` hint in -- `detail` names the field as missing, with no
    // `seen_at` key at all, rather than a stale or fabricated path.
    assert!(
        detail.get("seen_at").is_none(),
        "a short-circuited http call has no response body to hint seen_at from: {structured}"
    );
}
