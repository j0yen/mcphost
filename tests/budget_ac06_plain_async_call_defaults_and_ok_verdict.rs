//! PRD-mcphost-run-budget-governor
//! AC6 (P0) — Given a plain async `host.tool_call` with no `budget`, When it
//! runs, Then `budget.limits` equal the plan defaults and `verdict` is `ok`
//! on completion.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn plain_async_call_carries_plan_defaults_and_finishes_ok() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
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
                json!({"name": "echoer", "args": {"hello": "world"}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    let done = loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            break got;
        }
        assert!(Instant::now() < deadline, "run never finalized within 10s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(done["status"], json!("done"), "{done}");
    let budget = &done["progress"]["budget"];
    // free plan defaults (PRD-mcphost-run-budget-governor requirement 2).
    assert_eq!(budget["limits"]["max_child_calls"], json!(20), "{budget}");
    assert_eq!(budget["limits"]["max_est_tokens"], json!(200_000), "{budget}");
    assert_eq!(budget["limits"]["max_tool_latency_ms"], json!(30_000), "{budget}");
    assert_eq!(budget["limits"]["max_wall_ms"], json!(60_000), "{budget}");
    assert_eq!(budget["verdict"], json!("ok"), "{budget}");
}
