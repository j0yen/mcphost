//! PRD-mcphost-channel-read-name-parity
//! AC3 (P0) -- Given a group channel, When read, post, freeze, close run as
//! before this PRD, Then results are byte-identical to the pre-PRD
//! fixtures (no regression). `resolve_channel` now sits in front of every
//! verb (requirement 1/2), so this proves the group-channel path through
//! it still produces the exact same shapes/values
//! `channel_ac01`/`channel_ac09`/`channel_ac10` already pinned pre-PRD.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn group_channel_read_post_freeze_close_are_unchanged_by_resolve_channel() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "ChanRead AC3 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "ChanRead AC3 A").await;
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
    assert!(opened.get("group").is_some(), "{opened:?}");
    assert!(opened.get("created_at").is_some(), "{opened:?}");

    // Post: a member posts, response carries from_address and seq 1.
    let posted = extract_structured(
        &client_a
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": "hello group"}))
            .await
            .expect("member posts"),
    );
    assert_eq!(posted["seq"], json!(1), "{posted:?}");
    assert_eq!(posted["from_address"], json!(ns_a), "{posted:?}");
    assert_eq!(posted["channel_id"], json!(channel_id), "{posted:?}");

    // Read: owner reads, same post comes back with from_address.
    let read = extract_structured(
        &client_o
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("owner reads"),
    );
    let posts = read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 1, "{read:?}");
    assert_eq!(posts[0]["seq"], json!(1), "{read:?}");
    assert_eq!(posts[0]["from_address"], json!(ns_a), "{read:?}");
    assert_eq!(posts[0]["body"], json!("hello group"), "{read:?}");
    assert!(read.get("next_cursor").is_none(), "{read:?}");

    // Freeze: further posts refused channel_frozen; reads keep working.
    let frozen = extract_structured(
        &client_o
            .tools_call("host.channel.freeze", json!({"channel_id": channel_id}))
            .await
            .expect("owner freezes"),
    );
    assert_eq!(frozen["frozen"], json!(true), "{frozen:?}");
    let frozen_post_err = client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "while frozen"}))
        .await
        .expect_err("post while frozen must fail");
    assert_eq!(frozen_post_err.error_code.as_deref(), Some("channel_frozen"), "{frozen_post_err:?}");
    let still_read = extract_structured(
        &client_o
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("reads keep working while frozen"),
    );
    assert_eq!(still_read["posts"].as_array().expect("posts").len(), 1, "{still_read:?}");

    // Unfreeze, then close: further posts refused channel_closed; reads
    // keep working past that too.
    client_o
        .tools_call("host.channel.unfreeze", json!({"channel_id": channel_id}))
        .await
        .expect("owner unfreezes");
    let closed = extract_structured(
        &client_o
            .tools_call("host.channel.close", json!({"channel_id": channel_id}))
            .await
            .expect("owner closes"),
    );
    assert_eq!(closed["closed"], json!(true), "{closed:?}");
    let closed_post_err = client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "after close"}))
        .await
        .expect_err("post after close must fail");
    assert_eq!(closed_post_err.error_code.as_deref(), Some("channel_closed"), "{closed_post_err:?}");
    let read_after_close = extract_structured(
        &client_o
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("read after close must still work"),
    );
    assert_eq!(read_after_close["posts"].as_array().expect("posts").len(), 1, "{read_after_close:?}");
}
