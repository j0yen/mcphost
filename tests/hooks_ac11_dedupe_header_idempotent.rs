//! AC11 (P1) — Given `dedupe_header: "X-GitHub-Delivery"`, When the same
//! delivery id arrives twice, Then the second answers 202 with the first
//! run id and no second run exists.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn repeated_delivery_id_reuses_the_first_run_id() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC11 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dedupe_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "dedupe_hook",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
                "dedupe_header": "X-GitHub-Delivery",
            }),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    let url = format!("{}/hooks/{}/dedupe_hook", server.base_url, ns);

    let first = http
        .post(&url)
        .header("X-GitHub-Delivery", "delivery-123")
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hooks/... (first)");
    assert_eq!(first.status(), 202);
    let first_body: serde_json::Value = first.json().await.expect("json body");
    let first_run_id = first_body["run_id"].as_str().expect("run_id").to_string();

    let second = http
        .post(&url)
        .header("X-GitHub-Delivery", "delivery-123")
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hooks/... (second, same delivery id)");
    assert_eq!(second.status(), 202);
    let second_body: serde_json::Value = second.json().await.expect("json body");
    assert_eq!(
        second_body["run_id"], first_body["run_id"],
        "a repeated delivery id must answer with the original run id"
    );

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1, "no second run must exist: {runs:?}");
    assert_eq!(runs[0]["run_id"], json!(first_run_id));
}
