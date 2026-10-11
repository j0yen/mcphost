//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC5 (P0) -- Given `host.channel.read` with 0 posts after the cursor, When
//! called, Then `posts` is `[]` and `next_cursor` is absent; Given
//! `ack: true`, Then the stored member cursor is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn read(client: &McpClient, args: serde_json::Value) -> serde_json::Value {
    extract_structured(&client.tools_call("host.channel.read", args).await.expect("read"))
}

#[tokio::test]
async fn empty_read_has_no_next_cursor_and_ack_leaves_the_stored_cursor_alone() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Pgtr AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let opened = extract_structured(&client.tools_call("host.channel.open", json!({"name": "c"})).await.expect("open"));
    let channel = opened["channel_id"].as_str().expect("channel_id").to_string();

    // An empty channel: `[]`, no cursor key, and ack stores nothing.
    let empty = read(&client, json!({"channel_id": channel, "ack": true})).await;
    assert_eq!(empty["posts"], json!([]), "{empty:?}");
    assert!(empty.get("next_cursor").is_none(), "{empty:?}");

    for n in 1..=3 {
        client
            .tools_call("host.channel.post", json!({"channel": channel, "body": format!("p{n}")}))
            .await
            .expect("post");
    }

    // Page one of three at limit 2, acked: the stored cursor moves to seq 2.
    let page = read(&client, json!({"channel_id": channel, "limit": 2, "ack": true})).await;
    assert_eq!(page["posts"].as_array().expect("posts").len(), 2, "{page:?}");
    let cursor = page["next_cursor"].as_str().expect("a non-last page has next_cursor").to_string();

    // An explicit cursor past the end reads nothing; acking that empty read
    // must NOT move (or reset) the stored cursor, which stays at seq 2.
    let past_end = read(&client, json!({"channel_id": channel, "cursor": cursor, "limit": 100})).await;
    assert_eq!(past_end["posts"].as_array().expect("posts").len(), 1, "{past_end:?}");
    assert!(past_end.get("next_cursor").is_none(), "{past_end:?}");
    let legacy_end = read(&client, json!({"channel_id": channel, "cursor": "3", "ack": true})).await;
    assert_eq!(legacy_end["posts"], json!([]), "{legacy_end:?}");
    assert!(legacy_end.get("next_cursor").is_none(), "{legacy_end:?}");

    let resumed = read(&client, json!({"channel_id": channel})).await;
    let posts = resumed["posts"].as_array().expect("posts");
    assert_eq!(posts.len(), 1, "stored cursor must still be 2: {resumed:?}");
    assert_eq!(posts[0]["seq"], json!(3), "{resumed:?}");
}
