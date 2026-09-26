//! PRD-mcphost-run-result-overflow-to-state
//! AC4 (P0) — Given a run whose result is 10 KiB, When `host.runs.part
//! {run_id, n: 0}` is called, Then `parts == 1` and `data` equals the
//! inline result; When `n: 1`, Then `not_found`.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_small_result_reads_back_as_one_part_and_n1_is_not_found() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RunOverflow AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    let blob = "x".repeat(10 * 1024);
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "echoer", "args": {"blob": blob}, "async": true}),
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let done = done.expect("echo job must finish well within 5s");
    assert_eq!(done["result_ref"]["parts"], json!(1), "run: {done}");

    let part0 = extract_structured(
        &client
            .tools_call("host.runs.part", json!({"run_id": run_id, "n": 0}))
            .await
            .expect("part 0 ok"),
    );
    assert_eq!(part0["parts"], json!(1), "part0: {part0}");
    assert_eq!(part0["data"], done["result"], "part0: {part0}");

    let err = client
        .tools_call("host.runs.part", json!({"run_id": run_id, "n": 1}))
        .await
        .expect_err("part 1 must not exist for a single-part result");
    assert_eq!(err.error_code.as_deref(), Some("not_found"));
}
