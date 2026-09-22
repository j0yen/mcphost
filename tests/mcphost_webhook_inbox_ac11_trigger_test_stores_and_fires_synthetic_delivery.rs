//! PRD-mcphost-webhook-inbox
//! AC11 (P1) — Given `host.trigger.test` on a webhook trigger, When called,
//! Then one synthetic signed delivery is stored and fired.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn trigger_test_on_webhook_stores_and_fires_one_synthetic_delivery() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC11 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_payment", "kind": "webhook", "name": "pay"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let tested = extract_structured(
        &client
            .tools_call(
                "host.trigger.test",
                json!({"id": trigger_id, "body": {"amount": 777}}),
            )
            .await
            .expect("trigger.test"),
    );
    assert_eq!(tested["test"], json!(true));
    let run_id = tested["run_id"].as_str().expect("run_id present").to_string();

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "host.trigger.test must store exactly one synthetic delivery: {rows:?}");
    assert_eq!(rows[0]["body"], json!({"amount": 777}));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            assert_eq!(got["trigger"], json!("webhook"));
            break;
        }
        if Instant::now() >= deadline {
            panic!("run {run_id} never finished: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
