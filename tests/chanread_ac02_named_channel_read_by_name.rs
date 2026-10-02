//! PRD-mcphost-channel-read-name-parity
//! AC2 (P0) -- Given the same named channel as AC1, When
//! `host.channel.read("t")` runs by name, Then it returns the same posts
//! as reading by id.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn named_channel_read_by_name_returns_the_same_posts_as_by_id() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns, key) = signup(&server.base_url, "ChanRead AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let opened = extract_structured(
        &client.tools_call("host.channel.open", json!({"name": "t"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    client
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "fleet alert"}))
        .await
        .expect("post");

    let read_by_id = extract_structured(
        &client
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("read by id"),
    );
    let read_by_name = extract_structured(
        &client
            .tools_call("host.channel.read", json!({"channel_id": "t"}))
            .await
            .expect("read by name must succeed just like read by id"),
    );

    assert_eq!(read_by_name["posts"], read_by_id["posts"], "{read_by_name:?} vs {read_by_id:?}");
    assert_eq!(read_by_name["next_cursor"], read_by_id["next_cursor"], "{read_by_name:?}");
    let posts = read_by_name["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 1, "{read_by_name:?}");
    assert_eq!(posts[0]["seq"], json!(1), "{read_by_name:?}");
    assert_eq!(posts[0]["body"], json!("fleet alert"), "{read_by_name:?}");
}
