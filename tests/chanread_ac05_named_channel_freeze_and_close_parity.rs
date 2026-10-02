//! PRD-mcphost-channel-read-name-parity
//! AC5 (P0) -- Given a named channel, When `freeze(X)` then `post(X, ...)`
//! run, Then post fails `channel_frozen`; When `close(X)` then `read(X)`,
//! Then read still works (closed channels stay readable, matching
//! `errors.rs`'s documented rule) -- `freeze`/`close` previously only ever
//! matched a group channel's `group_id`, so this is new capability, not a
//! regression check.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn named_channel_freeze_blocks_post_and_close_leaves_read_working() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns, key) = signup(&server.base_url, "ChanRead AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let opened = extract_structured(
        &client.tools_call("host.channel.open", json!({"name": "t"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    client
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "before freeze"}))
        .await
        .expect("post before freeze");

    let frozen = extract_structured(
        &client
            .tools_call("host.channel.freeze", json!({"channel_id": channel_id}))
            .await
            .expect("freeze a named channel must now succeed"),
    );
    assert_eq!(frozen["frozen"], json!(true), "{frozen:?}");

    let post_err = client
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "while frozen"}))
        .await
        .expect_err("post to a frozen named channel must fail");
    assert_eq!(post_err.error_code.as_deref(), Some("channel_frozen"), "{post_err:?}");

    client
        .tools_call("host.channel.unfreeze", json!({"channel_id": channel_id}))
        .await
        .expect("unfreeze a named channel must now succeed");
    client
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "after unfreeze"}))
        .await
        .expect("post after unfreeze succeeds");

    let closed = extract_structured(
        &client
            .tools_call("host.channel.close", json!({"channel_id": channel_id}))
            .await
            .expect("close a named channel must now succeed"),
    );
    assert_eq!(closed["closed"], json!(true), "{closed:?}");

    let read = extract_structured(
        &client
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("read on a closed named channel must still work"),
    );
    let posts = read["posts"].as_array().expect("posts array");
    assert_eq!(posts.len(), 2, "{read:?}");
    assert_eq!(posts[0]["body"], json!("before freeze"), "{read:?}");
    assert_eq!(posts[1]["body"], json!("after unfreeze"), "{read:?}");
}
