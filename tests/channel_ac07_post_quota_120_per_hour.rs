//! PRD-mcphost-agent-channels
//! AC7 (P0) — Given the free plan's `channel_posts_per_hour` is 120, When
//! A posts 121 times within an hour, Then the 121st returns
//! `quota_exceeded` with `data.limit="channel_posts_per_hour"` and
//! `data.value=120`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn the_121st_post_in_an_hour_is_quota_exceeded() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC7 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Channel AC7 A").await;
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

    for n in 1..=120 {
        client_a
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": format!("m{n}")}))
            .await
            .unwrap_or_else(|e| panic!("post {n} should succeed under quota: {} {}", e.code, e.message));
    }

    let err = client_a
        .tools_call("host.channel.post", json!({"channel": channel_id, "body": "m121"}))
        .await
        .expect_err("121st post must be over quota");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!("channel_posts_per_hour"), "{err:?}");
    assert_eq!(err.data["value"], json!(120), "{err:?}");
}
