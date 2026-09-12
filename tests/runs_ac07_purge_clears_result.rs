//! AC7 (P0) — Given a done job, When `host.runs.purge(before_unix=now)`
//! runs, Then its result is gone from the store and `host.runs.get` reads
//! `done` with `result: null, purged: true`.
//!
//! Uses `echo` (no sandbox) dispatched as a job via `async: true` so the
//! test stays fast; the property under test is purge, not job execution
//! itself (already covered by `runs_ac01`).

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn purge_clears_a_done_runs_result() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "echoer", "args": {"msg": "hi"}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut done = None;
    while Instant::now() < deadline {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("done") {
            done = Some(got);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let done = done.expect("echo job must finish well within 5s");
    assert!(!done["result"].is_null(), "result must be present before purge: {done}");
    assert_eq!(done["purged"], json!(false));

    let now = mcphost::state::now_unix();
    let purge = extract_structured(
        &client
            .tools_call("host.runs.purge", json!({"before_unix": now}))
            .await
            .expect("purge ok"),
    );
    assert_eq!(purge["purged"], json!(1), "one run's result must be purged: {purge}");

    let after = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get ok"),
    );
    assert_eq!(after["status"], json!("done"), "status stays done: {after}");
    assert_eq!(after["result"], json!(null), "result must be gone: {after}");
    assert_eq!(after["purged"], json!(true), "purged flag must be set: {after}");
}
