//! PRD-mcphost-agent-channels
//! AC2 (P0) — Given tenant X is not a member, When X calls
//! `host.channel.read` or `host.channel.post` on the channel, Then both
//! return `channel_not_found` byte-identical to the response for a
//! random id.

use crate::common;
use common::{McpClient, RpcError, TestServer, extract_structured, signup};
use serde_json::json;

fn assert_same_error(a: &RpcError, b: &RpcError, what: &str) {
    assert_eq!(a.code, b.code, "{what}: jsonrpc code differs");
    assert_eq!(a.message, b.message, "{what}: message differs");
    assert_eq!(a.error_code.as_deref(), Some("channel_not_found"), "{what}: {a:?}");
    assert_eq!(a.error_code, b.error_code, "{what}: error_code differs");
    assert_eq!(a.data, b.data, "{what}: data differs");
}

#[tokio::test]
async fn non_member_gets_channel_not_found_byte_identical_to_a_random_id() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC2 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Channel AC2 A").await;
    let (_ns_x, key_x) = signup(&server.base_url, "Channel AC2 X").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_x = McpClient::with_bearer(&server.base_url, &key_x);

    client_o
        .tools_call("host.group.create", json!({"name": "nightly"}))
        .await
        .expect("group.create");
    client_o
        .tools_call("host.group.add", json!({"name": "nightly", "namespace": ns_a}))
        .await
        .expect("group.add");
    let opened = extract_structured(
        &client_o
            .tools_call("host.channel.open", json!({"group": "nightly"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();
    client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "hi"}))
        .await
        .expect("A posts");

    let random_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    let read_x = client_x
        .tools_call("host.channel.read", json!({"channel_id": channel_id}))
        .await
        .expect_err("X is not a member");
    let read_random = client_x
        .tools_call("host.channel.read", json!({"channel_id": random_id}))
        .await
        .expect_err("random id");
    assert_same_error(&read_x, &read_random, "read");

    let post_x = client_x
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "hi"}))
        .await
        .expect_err("X is not a member");
    let post_random = client_x
        .tools_call("host.channel.post", json!({"channel": random_id, "body": "hi"}))
        .await
        .expect_err("random id");
    assert_same_error(&post_x, &post_random, "post");
}
