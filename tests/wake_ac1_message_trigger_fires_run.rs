//! PRD-mcphost-agent-wake
//! AC1 (P0) — Given tenant R published tool handle_msg and called
//! host.trigger.set(tool="handle_msg", kind="message"), When tenant S sends
//! R a message, Then within 5s host.runs.list(trigger="message") for R
//! shows one run whose args equal the message envelope (message_id,
//! thread_id, from=S's address, body).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn message_trigger_fires_a_run_with_the_envelope_as_args() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC1 Recipient").await;
    let (ns_s, key_s) = signup(&server.base_url, "Wake AC1 Sender").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);

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
    assert_eq!(set["kind"], json!("message"), "{set:?}");
    assert_eq!(set["from"], serde_json::Value::Null, "{set:?}");

    let sent = extract_structured(
        &client_s
            .tools_call("host.msg.send", json!({"to": [ns_r], "body": "wake up"}))
            .await
            .expect("send"),
    );
    let message_id = sent["message_id"].as_str().expect("message_id").to_string();
    let thread_id = sent["thread_id"].as_str().expect("thread_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let list = extract_structured(
            &client_r
                .tools_call("host.runs.list", json!({"trigger": "message"}))
                .await
                .expect("runs.list"),
        );
        let runs = list["runs"].as_array().expect("runs array");
        assert!(runs.len() <= 1, "expected at most one message-triggered run: {list:?}");
        if let Some(run) = runs.first()
            && run["status"] == json!("done")
        {
            let got = extract_structured(
                &client_r
                    .tools_call("host.runs.get", json!({"run_id": run["run_id"].as_str().unwrap()}))
                    .await
                    .expect("runs.get"),
            );
            assert_eq!(got["result"]["message_id"], json!(message_id), "{got:?}");
            assert_eq!(got["result"]["thread_id"], json!(thread_id), "{got:?}");
            assert_eq!(got["result"]["from"], json!(ns_s), "{got:?}");
            assert_eq!(got["result"]["body"], json!("wake up"), "{got:?}");
            assert_eq!(got["trigger"], json!("message"), "{got:?}");
            return;
        }
        assert!(Instant::now() < deadline, "message-triggered run never finished: {list:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
