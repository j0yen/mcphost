//! PRD-mcphost-channel-read-name-parity
//! AC1 (P0) -- Given `host.channel.open(name: "t")` returning id X, When
//! `host.channel.post(X, body)` then `host.channel.read(X)` run, Then read
//! returns the post with `seq` 1 and a `next_cursor`. This is the exact
//! fleet-board dogfood sequence the PRD's grounding section reproduced:
//! `open(name) -> post(id) -> read(id)` used to fail `channel_not_found`
//! because `read` only ever ran the group-channel lookup.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn named_channel_post_then_read_by_id_returns_the_post() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns, key) = signup(&server.base_url, "ChanRead AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let opened = extract_structured(
        &client.tools_call("host.channel.open", json!({"name": "t"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    let posted = extract_structured(
        &client
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": "fleet alert"}))
            .await
            .expect("post"),
    );
    assert_eq!(posted["seq"], json!(1), "{posted:?}");

    let read = extract_structured(
        &client
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("read by id must succeed, not channel_not_found"),
    );
    let posts = read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 1, "{read:?}");
    assert_eq!(posts[0]["seq"], json!(1), "{read:?}");
    assert_eq!(posts[0]["body"], json!("fleet alert"), "{read:?}");
    assert!(read.get("next_cursor").is_some(), "{read:?}");
    assert_eq!(read["next_cursor"], json!(1), "{read:?}");
}
