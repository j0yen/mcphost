//! PRD-mcphost-run-result-overflow-to-state
//! AC3 (P0) — Given `host.progress {run_id, counters: {items_processed:
//! 500}}` then `{items_processed: 400}`, When the second call is made, Then
//! it returns `validation` and `host.runs.get` shows 500.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_lower_counter_is_rejected_and_the_higher_value_survives() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RunOverflow AC3 Tenant").await;
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
                json!({"name": "echoer", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let first = extract_structured(
        &client
            .tools_call(
                "host.progress",
                json!({"run_id": run_id, "counters": {"items_processed": 500}}),
            )
            .await
            .expect("first progress call ok"),
    );
    assert_eq!(first["counters"]["items_processed"], json!(500), "first: {first}");

    let err = client
        .tools_call(
            "host.progress",
            json!({"run_id": run_id, "counters": {"items_processed": 400}}),
        )
        .await
        .expect_err("a lower counter must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("validation"));

    let got = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get ok"),
    );
    assert_eq!(
        got["counters"]["items_processed"],
        json!(500),
        "the rejected lower value must not overwrite the stored 500: {got}"
    );
}
