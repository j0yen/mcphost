//! PRD-mcphost-job-timeline-tool AC1 (P0) -- Given a completed 3-step chain
//! run, When `host.job_timeline(run_id)` is called, Then `steps` has 3 entries
//! in step order, each with `started_unix` <= `finished_unix`, `duration_ms` >=
//! 0, `status: done`, `bytes_out` > 0, and `totals.wall_ms` >= every step's
//! `duration_ms`.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn jtl_ac01() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "JTL1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    for name in ["step1", "step2", "step3"] {
        publish(&client, name, "echo", json!({"schema": schema})).await;
    }
    let chain = publish(
        &client,
        "pipeline",
        "chain",
        json!({"steps": [
            {"tool": "step1", "args": {"n": "$.input.n"}},
            {"tool": "step2", "args": {"m": "$.prev.result.n"}},
            {"tool": "step3", "args": {"k": "$.steps[1].result.m"}}
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({"n": 5})).await.expect("chain call");

    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let run_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();

    // The alias `host.job_timeline` and the canonical `host.job.timeline` agree.
    let timeline = extract_structured(
        &client.tools_call("host.job_timeline", json!({"run_id": run_id})).await.expect("timeline"),
    );
    let canonical = extract_structured(
        &client.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect("timeline canonical"),
    );
    assert_eq!(timeline["steps"].as_array().map(Vec::len), canonical["steps"].as_array().map(Vec::len));

    let steps = timeline["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "{timeline}");
    let tools: Vec<&str> = steps.iter().map(|s| s["tool"].as_str().unwrap().rsplit('.').next().unwrap()).collect();
    assert_eq!(tools, vec!["step1", "step2", "step3"]);
    let wall_ms = timeline["totals"]["wall_ms"].as_i64().expect("wall_ms");
    for (i, s) in steps.iter().enumerate() {
        assert_eq!(s["step"], json!(i + 1), "{s}");
        assert!(s["started_unix"].as_i64().unwrap() <= s["finished_unix"].as_i64().unwrap(), "{s}");
        let duration = s["duration_ms"].as_i64().expect("duration_ms");
        assert!(duration >= 0 && wall_ms >= duration, "wall {wall_ms} vs step {duration}");
        assert_eq!(s["status"], json!("done"), "{s}");
        assert!(s["bytes_out"].as_i64().unwrap() > 0, "{s}");
        assert!(s["error_class"].is_null() && s["error_family"].is_null(), "{s}");
    }
    assert_eq!(timeline["totals"]["steps"], json!(3));
    assert_eq!(timeline["totals"]["done"], json!(3));

    // Every JSON key is in the field table and every table row is a key.
    let mut keys: Vec<&str> = steps[0].as_object().unwrap().keys().map(String::as_str).collect();
    let mut table: Vec<&str> = mcphost::timeline::TIMELINE_FIELDS.iter().map(|(f, _)| *f).collect();
    keys.sort_unstable();
    table.sort_unstable();
    assert_eq!(keys, table, "timeline entry fields drifted from TIMELINE_FIELDS");

    let missing = extract_structured_err(&client, "run_nope").await;
    assert_eq!(missing, "run_not_found");
}

async fn extract_structured_err(client: &common::McpClient, run_id: &str) -> String {
    let err = client.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect_err("unknown run");
    if format!("{err:?}").contains("run_not_found") { "run_not_found".to_string() } else { String::new() }
}
