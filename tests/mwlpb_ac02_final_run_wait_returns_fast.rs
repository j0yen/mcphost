//! PRD-mcphost-msg-wait-long-poll-contract-b
//! AC2 (P1) — Given a run already in a final state, When `host.runs.wait`
//! is called, Then it returns in < 100 ms with `timed_out: false`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Instant;

#[tokio::test]
async fn already_final_run_wait_returns_under_100ms_not_timed_out() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "mwlpb ac02 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.tool_publish", json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let enqueue = extract_structured(
        &client.tools_call("host.tool_call", json!({"name": "echoer", "args": {}, "async": true})).await.expect("enqueue"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    // First wait absorbs the executor; afterwards the run is final.
    let first = extract_structured(&client.tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 20})).await.expect("first wait"));
    assert_eq!(first["status"], json!("done"), "{first}");

    // Best of a few samples absorbs HTTP/scheduler jitter; the wait itself has no sleep.
    let mut best = u128::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let body = extract_structured(&client.tools_call("host.runs.wait", json!({"run_id": run_id})).await.expect("wait"));
        best = best.min(started.elapsed().as_millis());
        assert_eq!(body["timed_out"], json!(false), "{body}");
        assert!(body["waited_ms"].as_u64().is_some_and(|ms| ms < 100), "{body}");
    }
    assert!(best < 100, "best wall {best} ms");
}
