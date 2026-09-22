//! PRD-mcphost-agent-channels
//! AC4 (P0) — Given six members each posting 100 messages concurrently,
//! When every member reads in a loop with `ack=true` until `next_cursor`
//! stops advancing, Then each member has 600 distinct `seq` values (its
//! own included) with no gaps.

use std::collections::HashSet;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const MEMBERS: usize = 6;
const POSTS_PER_MEMBER: i64 = 100;
const TOTAL_POSTS: i64 = MEMBERS as i64 * POSTS_PER_MEMBER;

async fn post_n(base_url: String, key: String, channel_id: String, n: i64) {
    let client = McpClient::with_bearer(&base_url, &key);
    for i in 0..n {
        client
            .tools_call("host.channel.post", json!({"channel": channel_id, "body": format!("m{i}")}))
            .await
            .unwrap_or_else(|e| panic!("post failed: {} {}", e.code, e.message));
    }
}

async fn read_all_seqs(base_url: String, key: String, channel_id: String) -> HashSet<i64> {
    let client = McpClient::with_bearer(&base_url, &key);
    let mut seen = HashSet::new();
    let mut prev_cursor: Option<i64> = None;
    loop {
        let read = extract_structured(
            &client
                .tools_call(
                    "host.channel.read",
                    json!({"channel_id": channel_id, "ack": true, "limit": 100}),
                )
                .await
                .expect("read"),
        );
        for post in read["posts"].as_array().expect("posts array") {
            seen.insert(post["seq"].as_i64().expect("seq"));
        }
        let next_cursor = read["next_cursor"].as_i64().expect("next_cursor");
        if prev_cursor == Some(next_cursor) {
            break;
        }
        prev_cursor = Some(next_cursor);
    }
    seen
}

#[tokio::test]
async fn six_members_posting_concurrently_leave_no_dups_or_gaps() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC4 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");

    let mut member_keys = Vec::new();
    for i in 0..MEMBERS {
        let (ns, key) = signup(&server.base_url, &format!("Channel AC4 M{i}")).await;
        client_o
            .tools_call("host.group.add", json!({"name": "g", "namespace": ns}))
            .await
            .expect("group.add");
        member_keys.push(key);
    }

    let opened = extract_structured(
        &client_o.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    let post_handles: Vec<_> = member_keys
        .iter()
        .map(|key| {
            tokio::spawn(post_n(
                server.base_url.clone(),
                key.clone(),
                channel_id.clone(),
                POSTS_PER_MEMBER,
            ))
        })
        .collect();
    for h in post_handles {
        h.await.expect("post task panicked");
    }

    let read_handles: Vec<_> = member_keys
        .iter()
        .map(|key| {
            tokio::spawn(read_all_seqs(server.base_url.clone(), key.clone(), channel_id.clone()))
        })
        .collect();
    for h in read_handles {
        let seen = h.await.expect("read task panicked");
        assert_eq!(seen.len() as i64, TOTAL_POSTS, "expected {TOTAL_POSTS} distinct seqs, got {}", seen.len());
        let expected: HashSet<i64> = (1..=TOTAL_POSTS).collect();
        assert_eq!(seen, expected, "seq set has gaps or dups");
    }
}
