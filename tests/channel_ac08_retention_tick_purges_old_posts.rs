//! PRD-mcphost-agent-channels
//! AC8 (P0) — Given posts older than `channel_retention_days` and a
//! member whose cursor points before the horizon, When the housekeeping
//! tick runs and the member reads, Then old posts are gone and the read
//! starts at the first retained `seq` with no error.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const FREE_RETENTION_DAYS: i64 = 14;

async fn post(client: &McpClient, channel_id: &str, body: &str) -> String {
    let posted = extract_structured(
        &client
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": body}))
            .await
            .expect("post"),
    );
    posted["post_id"].as_str().expect("post_id").to_string()
}

#[tokio::test]
async fn housekeeping_tick_purges_posts_past_retention_and_reads_stay_error_free() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC8 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "Channel AC8 B").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    client_o
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_b}))
        .await
        .expect("group.add");
    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    // Seq 1, 2: read and ack now, so B's stored cursor sits at 2 -- below
    // the horizon the tick is about to create.
    let post1 = post(&client_b, &channel_id, "m1").await;
    let post2 = post(&client_b, &channel_id, "m2").await;
    let ack_read = extract_structured(
        &client_b
            .tools_call("host.channel.read", json!({"channel_id": channel_id, "ack": true}))
            .await
            .expect("B acks up through seq 2"),
    );
    assert_eq!(ack_read["next_cursor"], json!(2), "{ack_read:?}");

    // Seq 3, 4, 5: posted after the ack, so B's cursor stays at 2.
    let post3 = post(&client_b, &channel_id, "m3").await;
    post(&client_b, &channel_id, "m4").await;
    post(&client_b, &channel_id, "m5").await;

    // Posts 1-3 are older than the free plan's 14-day retention window;
    // posts 4-5 stay fresh.
    let now_ms = mcphost::state::now_unix_ms();
    let old_ms = now_ms - (FREE_RETENTION_DAYS + 6) * 86_400_000;
    for post_id in [&post1, &post2, &post3] {
        server
            .state
            .db
            .test_backdate_channel_post(post_id.clone(), old_ms)
            .await
            .expect("backdate");
    }

    mcphost::channels::tick_once(&server.state).await.expect("housekeeping tick");

    let read = extract_structured(
        &client_b
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("B reads after the tick, no error"),
    );
    let posts = read["posts"].as_array().expect("posts array");
    let seqs: Vec<i64> = posts.iter().map(|p| p["seq"].as_i64().expect("seq")).collect();
    assert_eq!(seqs, vec![4, 5], "old posts should be gone, read starts at the first retained seq: {read:?}");
}
