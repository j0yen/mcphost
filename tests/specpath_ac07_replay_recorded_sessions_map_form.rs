//! PRD-mcphost-spec-output-paths AC7 — Given the three recorded
//! 2026-09-08 sessions replayed with their specs rewritten to the map form,
//! When their recorded upstream bodies are applied, Then each declared
//! field appears at `result.payload.<field>`.
//!
//! Same caveat `tests/envelope_ac5_replay_recorded_sessions.rs` documents:
//! the measure ledger records `judge_reason` prose and scores, not the
//! literal request/response bytes exchanged with the upstream (the
//! transcript itself was not persisted), so this replays a call shape
//! representative of each session's own recorded defect (Problem
//! statement, lines naming `panel_rag_indexer_03` and
//! `panel_admin_agent_01`'s rejected map-form publishes, plus
//! `panel_integration_specialist_01`'s nested field), now rewritten to the
//! map form this PRD adds instead of the list form the envelope-contract
//! PRD's own replay used.

mod common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn rag_indexer_03_map_form_ingestion_status_lands_at_contract_path() {
    // Problem statement: `panel_rag_indexer_03` sent
    // `spec.outputs = {"ingestion_status": "$.json.ingestion_status"}`
    // against a test endpoint that echoes its request body under `json`
    // (the httpbin-style shape this persona's recorded task targets).
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "json": {"ingestion_status": "1200/1200 documents indexed"},
            "args": {},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "SpecPath AC7 rag_indexer_03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "ingestion_source_status",
                "kind": "http",
                "spec": {
                    "method": "GET",
                    "url": format!("{}/anything", upstream.uri()),
                    "args_schema": {"type": "object"},
                    "outputs": {"ingestion_status": "$.json.ingestion_status"},
                },
            }),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.ingestion_source_status"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["ingestion_status"],
        "1200/1200 documents indexed"
    );
}

#[tokio::test]
async fn admin_agent_01_map_form_state_readable_lands_at_contract_path() {
    // Problem statement: `panel_admin_agent_01` sent
    // `spec.outputs = {"state_readable": "$.slideshow.title", ...}` against
    // a test endpoint whose body nests the field under `slideshow` (the
    // classic public "slideshow" JSON test fixture this persona's recorded
    // task used).
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "slideshow": {"title": "Sample Slide Show"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "SpecPath AC7 admin_agent_01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "state_readable_tool",
                "kind": "http",
                "spec": {
                    "method": "GET",
                    "url": format!("{}/json", upstream.uri()),
                    "args_schema": {"type": "object"},
                    "outputs": {"state_readable": "$.slideshow.title"},
                },
            }),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.state_readable_tool"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["payload"]["state_readable"], "Sample Slide Show");
}

#[tokio::test]
async fn integration_specialist_01_map_form_bridge_status_lands_at_contract_path() {
    // Problem statement / envelope-contract PRD's own AC5 fixture:
    // `panel_integration_specialist_01`'s upstream nested `bridge_status`
    // one level under `data`; rewritten here to the map form.
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"bridge_status": "healthy"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "SpecPath AC7 integration_specialist_01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "bridge_map_form",
                "kind": "http",
                "spec": {
                    "method": "GET",
                    "url": format!("{}/v1/bridge", upstream.uri()),
                    "args_schema": {"type": "object"},
                    "outputs": {"bridge_status": "$.data.bridge_status"},
                },
            }),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.bridge_map_form"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["payload"]["bridge_status"], "healthy");
}
