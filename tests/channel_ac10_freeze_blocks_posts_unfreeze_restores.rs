//! PRD-mcphost-agent-channels
//! AC10 (P1) — Given O froze the channel, When A posts, Then
//! `channel_frozen`; and after `unfreeze`, Then the post succeeds with
//! the next `seq`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn freeze_blocks_posts_and_unfreeze_restores_them_at_the_next_seq() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC10 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Channel AC10 A").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    client_o
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_a}))
        .await
        .expect("group.add");
    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    let first = extract_structured(
        &client_a
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": "before freeze"}))
            .await
            .expect("A posts before freeze"),
    );
    assert_eq!(first["seq"], json!(1), "{first:?}");

    let frozen = extract_structured(
        &client_o
            .tools_call("host.channel.freeze", json!({"channel_id": channel_id}))
            .await
            .expect("O freezes"),
    );
    assert_eq!(frozen["frozen"], json!(true), "{frozen:?}");

    let post_err = client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "while frozen"}))
        .await
        .expect_err("posting to a frozen channel must fail");
    assert_eq!(post_err.error_code.as_deref(), Some("channel_frozen"), "{post_err:?}");

    let unfrozen = extract_structured(
        &client_o
            .tools_call("host.channel.unfreeze", json!({"channel_id": channel_id}))
            .await
            .expect("O unfreezes"),
    );
    assert_eq!(unfrozen["frozen"], json!(false), "{unfrozen:?}");

    let second = extract_structured(
        &client_a
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": "after unfreeze"}))
            .await
            .expect("A posts after unfreeze"),
    );
    assert_eq!(second["seq"], json!(2), "{second:?}");
}
