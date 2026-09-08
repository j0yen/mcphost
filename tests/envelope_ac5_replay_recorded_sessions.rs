//! PRD-mcphost-result-envelope-contract AC5 — Given the four recorded
//! 2026-09-08 sessions' call shapes replayed against the fix, When each
//! tool is called, Then each session's checked field is found at
//! `result.payload.<field>`.
//!
//! The measure ledger (`evidence/mcp-host/measure/0.26.3-20260908T065427Z/
//! ledger.jsonl`) records each session's `judge_reason` prose and scores,
//! not the literal request/response bytes exchanged with the upstream --
//! the transcript itself was not persisted. This test replays a call shape
//! *representative* of each session's own judge_reason (documented per
//! test below), which is the smallest faithful reconstruction available;
//! what it proves is the one thing Requirement 5 asks for: the specific
//! defect class the panel measured (a real, correct field the judge's gold
//! check could not find) is closed by the promotion this PRD adds.
//!
//! - `panel_integration_specialist_01` (0.5, "returned the required
//!   bridge_status field (nested in response data)") -- `http` kind,
//!   `bridge_status` nested one level under `data`.
//! - `panel_integration_specialist_02` (0.444, "despite the structural
//!   deviation in where the diagnosis field appears") -- `python` kind,
//!   `diagnosis` nested one level under an author-chosen key (`analysis`).
//! - `panel_workflow_orchestrator_01` (0.492, "real backend content
//!   containing the blocking_tool value... the gold check's literal field
//!   path was not matched") -- `python` kind (arbitrary nesting, unlike
//!   `http`'s fixed wrapper-key allowlist), `blocking_tool` nested under an
//!   author-chosen key (`orchestration`).
//! - `panel_rag_indexer_03` (0.462, "real, substantive ingestion status
//!   metrics... despite GOLD CHECK format mismatch") -- `http` kind,
//!   `ingestion_status` nested one level under `data` (the common REST
//!   monitoring-endpoint shape this persona's task targets).

mod common;
use common::{
    McpClient, TestServer, extract_structured, http_kind_registry, poll_until_ready,
    python_kind_registry, signup,
};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn integration_specialist_01_bridge_status_lands_at_contract_path() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"bridge_status": "healthy"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC5 integration_specialist_01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "bridge",
                "kind": "http",
                "spec": {
                    "method": "GET",
                    "url": format!("{}/v1/bridge", upstream.uri()),
                    "args_schema": {"type": "object"},
                    "outputs": ["bridge_status"],
                },
            }),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.bridge"), json!({}))
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["payload"]["bridge_status"], "healthy");
}

#[tokio::test]
async fn integration_specialist_02_diagnosis_lands_at_contract_path() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC5 integration_specialist_02").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "failure_diagnosis",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"analysis\": {\"diagnosis\": \"queue backpressure on worker-3\"}}\n",
                    "args_schema": {"type": "object"},
                    "outputs": ["diagnosis"],
                },
            }),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.failure_diagnosis"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["diagnosis"],
        "queue backpressure on worker-3"
    );
}

#[tokio::test]
async fn workflow_orchestrator_01_blocking_tool_lands_at_contract_path() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC5 workflow_orchestrator_01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "scheduling_visibility",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"orchestration\": {\"blocking_tool\": \"sync-job-42\"}}\n",
                    "args_schema": {"type": "object"},
                    "outputs": ["blocking_tool"],
                },
            }),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.scheduling_visibility"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);

    assert_eq!(structured["payload"]["blocking_tool"], "sync-job-42");
}

#[tokio::test]
async fn rag_indexer_03_ingestion_status_lands_at_contract_path() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"ingestion_status": "1200/1200 documents indexed"},
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC5 rag_indexer_03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "ingestion_source_status",
                "kind": "http",
                "spec": {
                    "method": "GET",
                    "url": format!("{}/v1/ingestion", upstream.uri()),
                    "args_schema": {"type": "object"},
                    "outputs": ["ingestion_status"],
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
