//! PRD-mcphost-agent-channels
//! AC9 (P0) — Given O closed the channel, When A posts, Then
//! `channel_closed`; and When B reads, Then existing posts are returned.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn close_blocks_further_posts_but_reads_keep_working() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC9 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Channel AC9 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Channel AC9 B").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    for ns in [&ns_a, &ns_b] {
        client_o
            .tools_call("host.group.add", json!({"name": "g", "namespace": ns}))
            .await
            .expect("group.add");
    }
    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "before close"}))
        .await
        .expect("A posts before close");

    let closed = extract_structured(
        &client_o
            .tools_call("host.channel.close", json!({"channel_id": channel_id}))
            .await
            .expect("O closes"),
    );
    assert_eq!(closed["closed"], json!(true), "{closed:?}");

    let post_err = client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "after close"}))
        .await
        .expect_err("posting to a closed channel must fail");
    assert_eq!(post_err.error_code.as_deref(), Some("channel_closed"), "{post_err:?}");

    let read = extract_structured(
        &client_b
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("B still reads a closed channel"),
    );
    let posts = read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 1, "{read:?}");
    assert_eq!(posts[0]["body"], json!("before close"), "{read:?}");
}
