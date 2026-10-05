//! PRD-mcphost-event-trigger-self-test
//! AC7 (P0) — Given a done run whose result is 300 KiB (two parts), When
//! host.runs.list(include_result=true) is called, Then result is null and
//! result_ref.parts == 2.
//!
//! A 300 KiB result never actually overflows the executor's own
//! MAX_TOOL_OUTPUT_BYTES (1 MiB) chunking gate, so there is no live-pipeline
//! way to produce a genuine 2-part row at that size; this exercises
//! host.runs.list's own inlining decision directly against a row/parts
//! shape built the same way PRD-mcphost-run-result-overflow-to-state's own
//! `wake_ac4_jobs_concurrent_rejects.rs` pokes a run row straight into the
//! db for a scenario the live pipeline can't produce on demand.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_two_part_result_stays_null_in_runs_list_even_with_include_result() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "SelfTest AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db lookup")
        .expect("tenant exists");

    let run_id = "selftest-ac7-multipart-run".to_string();
    server
        .state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            "multipart_tool".to_string(),
            "event".to_string(),
            None,
            None,
            300,
            "{}".to_string(),
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("insert_queued_run");

    let bytes = 300 * 1024;
    let result_ref = json!({"parts": 2, "bytes": bytes, "content_type": "application/json"}).to_string();
    server
        .state
        .db
        .finalize_run(
            run_id.clone(),
            tenant.id,
            "done".to_string(),
            Some(result_ref),
            None,
            None,
            mcphost::state::now_unix(),
            10,
        )
        .await
        .expect("finalize_run");

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"include_result": true}))
            .await
            .expect("runs.list include_result=true"),
    );
    let row = listed["runs"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["run_id"] == json!(run_id)))
        .unwrap_or_else(|| panic!("run {run_id} not in runs.list: {listed}"));
    assert_eq!(row["result"], serde_json::Value::Null, "row: {row}");
    assert_eq!(row["result_ref"]["parts"], json!(2), "row: {row}");
}
