//! PRD-mcphost-run-budget-governor
//! AC9 (P1) — Given `host.runs.list(verdict: "exceeded")`, When called,
//! Then only runs with `budget_exceeded` return.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use std::time::{Duration, Instant};

async fn wait_finalized(client: &common::McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get ok"),
        );
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            return got;
        }
        assert!(Instant::now() < deadline, "run {run_id} never finalized within 15s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn verdict_exceeded_returns_only_budget_exceeded_runs() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    // Run A: an ordinary async call that finishes ok (verdict ok).
    let enqueue_ok = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "echoer", "args": {}, "async": true}))
            .await
            .expect("enqueue ok"),
    );
    let run_ok = enqueue_ok["run_id"].as_str().expect("run_id").to_string();
    let done_ok = wait_finalized(&client, &run_ok).await;
    assert_eq!(done_ok["status"], json!("done"), "{done_ok}");
    assert_eq!(done_ok["progress"]["budget"]["verdict"], json!("ok"), "{done_ok}");

    // Run B: a chain that overruns its own max_child_calls (verdict
    // exceeded, error_class budget_exceeded).
    let schema = json!({"type": "object"});
    for i in 0..3 {
        publish(&client, &format!("step{i}"), "echo", json!({"schema": schema})).await;
    }
    let steps: Vec<serde_json::Value> =
        (0..3).map(|i| json!({"tool": format!("step{i}"), "args": {"n": i}})).collect();
    let chain = publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    assert_eq!(chain, format!("{ns}.pipeline"));
    let enqueue_exceeded = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({
                    "name": "pipeline",
                    "args": {},
                    "async": true,
                    "budget": {"max_child_calls": 2},
                }),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_exceeded = enqueue_exceeded["run_id"].as_str().expect("run_id").to_string();
    let done_exceeded = wait_finalized(&client, &run_exceeded).await;
    assert_eq!(done_exceeded["status"], json!("error"), "{done_exceeded}");
    assert_eq!(done_exceeded["error_class"], json!("budget_exceeded"), "{done_exceeded}");
    assert_eq!(done_exceeded["progress"]["budget"]["verdict"], json!("exceeded"), "{done_exceeded}");

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"verdict": "exceeded"}))
            .await
            .expect("runs.list ok"),
    );
    let run_ids: Vec<&str> = listed["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .map(|r| r["run_id"].as_str().expect("run_id"))
        .collect();
    assert_eq!(
        run_ids,
        vec![run_exceeded.as_str()],
        "only the budget_exceeded run must be listed: {listed}"
    );
}
