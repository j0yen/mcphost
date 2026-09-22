//! PRD-mcphost-agent-channels
//! AC5 (P0) — Given O removed D from the group, When D reads or posts,
//! Then `channel_not_found`; and When A reads, Then D's earlier posts
//! are still present with D's `from_address`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn removed_member_loses_access_but_its_posts_remain() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC5 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Channel AC5 A").await;
    let (ns_d, key_d) = signup(&server.base_url, "Channel AC5 D").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_d = McpClient::with_bearer(&server.base_url, &key_d);

    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    client_o
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_a}))
        .await
        .expect("group.add a");
    client_o
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_d}))
        .await
        .expect("group.add d");
    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    client_d
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "d was here"}))
        .await
        .expect("D posts before removal");

    client_o
        .tools_call("host.group.remove", json!({"name": "g", "namespace": ns_d}))
        .await
        .expect("group.remove d");

    let read_err = client_d
        .tools_call("host.channel.read", json!({"channel_id": channel_id}))
        .await
        .expect_err("removed D can't read");
    assert_eq!(read_err.error_code.as_deref(), Some("channel_not_found"), "{read_err:?}");

    let post_err = client_d
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "still here?"}))
        .await
        .expect_err("removed D can't post");
    assert_eq!(post_err.error_code.as_deref(), Some("channel_not_found"), "{post_err:?}");

    let a_read = extract_structured(
        &client_a
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("A still reads"),
    );
    let posts = a_read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 1, "{a_read:?}");
    assert_eq!(posts[0]["from_address"], json!(ns_d), "{a_read:?}");
    assert_eq!(posts[0]["body"], json!("d was here"), "{a_read:?}");
}
