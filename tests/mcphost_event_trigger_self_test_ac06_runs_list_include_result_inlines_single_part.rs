//! PRD-mcphost-event-trigger-self-test
//! AC6 (P0) — Given a done run whose result is 1 KiB, When
//! host.runs.list(trigger="event", include_result=true) is called, Then
//! that row's result equals host.runs.get(run_id).result; and When called
//! without include_result, Then result is null and the row is
//! byte-identical to today's shape.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn runs_list_include_result_inlines_a_single_part_result_and_defaults_to_null() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "reflect", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "reflect",
                    "kind": "event",
                    "verify": {"scheme": "none", "allow_unverified": true},
                }),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    // ~1 KiB body -> the echo tool's own result (event.body plus wrapper)
    // is comfortably a single inline part.
    let blob = "x".repeat(1024);
    let tested = extract_structured(
        &client
            .tools_call("host.trigger.test", json!({"id": trigger_id, "body": {"blob": blob}}))
            .await
            .expect("trigger.test"),
    );
    let run_id = tested["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            break;
        }
        assert!(Instant::now() < deadline, "run {run_id} never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let expected_result = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get"),
    )["result"]
        .clone();
    assert_ne!(expected_result, serde_json::Value::Null, "the run's own result must be non-null");

    let with_result = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event", "include_result": true}))
            .await
            .expect("runs.list include_result=true"),
    );
    let row = with_result["runs"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["run_id"] == json!(run_id)))
        .unwrap_or_else(|| panic!("run {run_id} not in runs.list: {with_result}"));
    assert_eq!(row["result"], expected_result, "row: {row}");

    let without_result = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event"}))
            .await
            .expect("runs.list default"),
    );
    let row_default = without_result["runs"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["run_id"] == json!(run_id)))
        .unwrap_or_else(|| panic!("run {run_id} not in runs.list: {without_result}"));
    assert_eq!(row_default["result"], serde_json::Value::Null, "row: {row_default}");

    // Byte-identical shape otherwise: every other field matches the
    // include_result=true row once `result` itself is excluded.
    let mut a = row.clone();
    let mut b = row_default.clone();
    a.as_object_mut().unwrap().remove("result");
    b.as_object_mut().unwrap().remove("result");
    assert_eq!(a, b, "only `result` may differ between the two calls");
}
