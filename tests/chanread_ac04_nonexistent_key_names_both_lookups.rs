//! PRD-mcphost-channel-read-name-parity
//! AC4 (P0) -- Given a nonexistent key, When any channel verb runs, Then
//! the error class is `channel_not_found`, `data.key` echoes the input,
//! and the message names both lookups `resolve_channel` tried.

use crate::common;
use common::{McpClient, RpcError, TestServer, signup};
use serde_json::json;

fn assert_channel_not_found_names_both_lookups(err: &RpcError, key: &str, what: &str) {
    assert_eq!(err.error_code.as_deref(), Some("channel_not_found"), "{what}: {err:?}");
    assert_eq!(err.data["key"], json!(key), "{what}: data.key must echo the input: {err:?}");
    assert!(
        err.message.contains("group channel") && err.message.contains("by id or name"),
        "{what}: message must name both lookups resolve_channel tried: {err:?}"
    );
}

#[tokio::test]
async fn nonexistent_key_gets_channel_not_found_with_echoed_key_and_both_lookups_named() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns, key) = signup(&server.base_url, "ChanRead AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let bogus = "nonexistent-channel-key";

    let read_err = client
        .tools_call("host.channel.read", json!({"channel_id": bogus}))
        .await
        .expect_err("read on a nonexistent key must fail");
    assert_channel_not_found_names_both_lookups(&read_err, bogus, "read");

    let post_err = client
        .tools_call("host.channel.post", json!({"channel": bogus, "body": "hi"}))
        .await
        .expect_err("post on a nonexistent key must fail");
    assert_channel_not_found_names_both_lookups(&post_err, bogus, "post");

    let freeze_err = client
        .tools_call("host.channel.freeze", json!({"channel_id": bogus}))
        .await
        .expect_err("freeze on a nonexistent key must fail");
    assert_channel_not_found_names_both_lookups(&freeze_err, bogus, "freeze");

    let close_err = client
        .tools_call("host.channel.close", json!({"channel_id": bogus}))
        .await
        .expect_err("close on a nonexistent key must fail");
    assert_channel_not_found_names_both_lookups(&close_err, bogus, "close");
}
