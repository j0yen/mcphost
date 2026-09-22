//! PRD-mcphost-agent-channels
//! AC3 (P0) — Given B read up to `seq` 10 with `ack=true`, When C posts
//! twice and B calls `host.channel.read(channel_id)` with no cursor,
//! Then B receives exactly `seq` 11 and 12.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ack_true_stores_cursor_so_the_next_read_resumes_after_it() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC3 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "Channel AC3 B").await;
    let (ns_c, key_c) = signup(&server.base_url, "Channel AC3 C").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);

    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    client_o
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_b}))
        .await
        .expect("group.add b");
    client_o
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_c}))
        .await
        .expect("group.add c");
    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    for n in 1..=10 {
        let posted = extract_structured(
            &client_c
                .tools_call("host.channel.post", json!({"channel": channel_id, "body": format!("post {n}")}))
                .await
                .expect("C posts"),
        );
        assert_eq!(posted["seq"], json!(n), "{posted:?}");
    }

    let first_read = extract_structured(
        &client_b
            .tools_call("host.channel.read", json!({"channel_id": channel_id, "ack": true}))
            .await
            .expect("B's first read"),
    );
    assert_eq!(first_read["posts"].as_array().expect("posts").len(), 10, "{first_read:?}");
    assert_eq!(first_read["next_cursor"], json!(10), "{first_read:?}");

    for n in 11..=12 {
        let posted = extract_structured(
            &client_c
                .tools_call("host.channel.post", json!({"channel": channel_id, "body": format!("post {n}")}))
                .await
                .expect("C posts"),
        );
        assert_eq!(posted["seq"], json!(n), "{posted:?}");
    }

    let second_read = extract_structured(
        &client_b
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("B's second read, no cursor"),
    );
    let posts = second_read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 2, "{second_read:?}");
    assert_eq!(posts[0]["seq"], json!(11), "{second_read:?}");
    assert_eq!(posts[1]["seq"], json!(12), "{second_read:?}");
    assert_eq!(second_read["next_cursor"], json!(12), "{second_read:?}");
}
