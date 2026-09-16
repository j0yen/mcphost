//! PRD-mcphost-agent-wake
//! AC7 (P0) — Given host.trigger.test(trigger_id) on a message trigger,
//! When it runs, Then the tool receives an envelope with test:true and no
//! messages row is created.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn trigger_test_on_a_message_trigger_uses_a_synthetic_envelope_and_stores_no_message() {
    let server = TestServer::start().await;
    let (_ns_r, key_r) = signup(&server.base_url, "Wake AC7 Recipient").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);

    client_r
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client_r
            .tools_call("host.trigger.set", json!({"tool": "handle_msg", "kind": "message"}))
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let before_inbox = extract_structured(
        &client_r.tools_call("host.msg.inbox", json!({})).await.expect("inbox before"),
    );
    assert_eq!(before_inbox["messages"], json!([]), "{before_inbox:?}");

    let tested = extract_structured(
        &client_r
            .tools_call(
                "host.trigger.test",
                json!({"id": trigger_id, "body": "synthetic ping", "from": "@tester"}),
            )
            .await
            .expect("trigger.test"),
    );
    assert_eq!(tested["test"], json!(true), "{tested:?}");
    let run_id = tested["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client_r
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            assert_eq!(got["test"], json!(true), "{got:?}");
            assert_eq!(got["result"]["test"], json!(true), "{got:?}");
            assert_eq!(got["result"]["from"], json!("@tester"), "{got:?}");
            assert_eq!(got["result"]["body"], json!("synthetic ping"), "{got:?}");
            assert!(
                got["result"]["message_id"].as_str().is_some_and(|s| s.starts_with("test-")),
                "a synthetic envelope's message_id must never collide with a real one: {got:?}"
            );
            break;
        }
        assert!(Instant::now() < deadline, "trigger.test run never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let after_inbox = extract_structured(
        &client_r.tools_call("host.msg.inbox", json!({})).await.expect("inbox after"),
    );
    assert_eq!(
        after_inbox["messages"], json!([]),
        "host.trigger.test must create no messages row: {after_inbox:?}"
    );
}
