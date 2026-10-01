//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC7 (P1) — Given two reads with `ack=True`, When the second runs, Then
//! it returns only posts after the first read's `next_cursor`.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_channel_read_ack_resumes_from_the_first_reads_cursor() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Chanbridge AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.group.create", json!({"name": "ack"}))
        .await
        .expect("group.create");
    let opened = extract_structured(
        &client
            .tools_call("host.channel.open", json!({"group": "ack"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    client
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "first"}))
        .await
        .expect("post first");

    let spec = json!({
        "source": "from mcphost import channel\ndef main(args):\n    return channel.read(args[\"cid\"], ack=True)\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "reader", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.reader");
    let first_read = extract_structured(
        &poll_until_ready(&client, &qualified, json!({"cid": channel_id}), Duration::from_secs(10))
            .await
            .unwrap_or_else(|e| panic!("first read must succeed: {} {}", e.code, e.message)),
    );
    let first_posts = first_read["posts"].as_array().expect("posts array");
    assert_eq!(first_posts.len(), 1, "{first_read:?}");
    assert_eq!(first_posts[0]["body"], json!("first"), "{first_read:?}");
    let first_cursor = first_read["next_cursor"].clone();

    client
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "second"}))
        .await
        .expect("post second");

    let second_read = extract_structured(
        &client
            .tools_call(&qualified, json!({"cid": channel_id}))
            .await
            .expect("second read must succeed"),
    );
    let second_posts = second_read["posts"].as_array().expect("posts array");
    assert_eq!(second_posts.len(), 1, "{second_read:?}");
    assert_eq!(second_posts[0]["body"], json!("second"), "{second_read:?}");
    assert_ne!(second_read["next_cursor"], first_cursor, "{second_read:?}");
}
