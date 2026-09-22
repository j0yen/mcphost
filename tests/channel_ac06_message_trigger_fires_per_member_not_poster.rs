//! PRD-mcphost-agent-channels
//! AC6 (P0) — Given B and C bound `handle_msg` with
//! `host.trigger.set(kind="message", channel_id=…)`, When A posts once,
//! Then exactly two runs exist for that `message_id` (B's and C's) and A
//! has none.

use std::time::{Duration, Instant};

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn wait_for_one_done_run(client: &McpClient) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let list = extract_structured(
            &client.tools_call("host.runs.list", json!({"trigger": "message"})).await.expect("runs.list"),
        );
        let runs = list["runs"].as_array().expect("runs array").clone();
        assert!(runs.len() <= 1, "expected at most one message-triggered run: {list:?}");
        if let Some(run) = runs.first()
            && run["status"] == json!("done")
        {
            return extract_structured(
                &client
                    .tools_call("host.runs.get", json!({"run_id": run["run_id"].as_str().unwrap()}))
                    .await
                    .expect("runs.get"),
            );
        }
        assert!(Instant::now() < deadline, "message-triggered run never finished: {list:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn channel_post_fires_exactly_one_run_per_bound_member_excluding_poster() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC6 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Channel AC6 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Channel AC6 B").await;
    let (ns_c, key_c) = signup(&server.base_url, "Channel AC6 C").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);

    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    for ns in [&ns_a, &ns_b, &ns_c] {
        client_o
            .tools_call("host.group.add", json!({"name": "g", "namespace": ns}))
            .await
            .expect("group.add");
    }
    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    for client in [&client_b, &client_c] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish handle_msg");
        let set = extract_structured(
            &client
                .tools_call(
                    "host.trigger.set",
                    json!({"tool": "handle_msg", "kind": "message", "channel_id": channel_id}),
                )
                .await
                .expect("trigger.set"),
        );
        assert_eq!(set["channel_id"], json!(channel_id), "{set:?}");
    }

    let posted = extract_structured(
        &client_a
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": "run your checks"}))
            .await
            .expect("A posts"),
    );
    let post_id = posted["post_id"].as_str().expect("post_id").to_string();

    for client in [&client_b, &client_c] {
        let got = wait_for_one_done_run(client).await;
        assert_eq!(got["result"]["message_id"], json!(post_id), "{got:?}");
        assert_eq!(got["result"]["channel_id"], json!(channel_id), "{got:?}");
        assert_eq!(got["result"]["from"], json!(ns_a), "{got:?}");
        assert_eq!(got["trigger"], json!("message"), "{got:?}");
    }

    let a_runs = extract_structured(
        &client_a.tools_call("host.runs.list", json!({"trigger": "message"})).await.expect("runs.list"),
    );
    assert_eq!(a_runs["runs"].as_array().expect("runs array").len(), 0, "{a_runs:?}");
}
