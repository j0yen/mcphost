//! PRD-mcphost-agent-channels
//! AC1 (P0) — Given owner O's group `nightly` has members A..F, When O
//! calls `host.channel.open(group="nightly")` and A posts "index
//! rebuilt", Then each of B..F reading `host.channel.read(channel_id)`
//! receives that post with `seq` 1 and `from_address` equal to A's
//! namespace.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn channel_open_broadcasts_a_post_to_every_member() {
    // 7 signups (owner + 6 members) is over the default 5/hour signup
    // limit -- same `start_with_signup_rate_limit` bump every multi-tenant
    // test in this suite already uses (e.g. tests/ac09_signup_rate_limit.rs).
    let server = TestServer::start_with_signup_rate_limit(100).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Channel AC1 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);

    let mut members = Vec::new();
    for name in ["A", "B", "C", "D", "E", "F"] {
        let (ns, key) = signup(&server.base_url, &format!("Channel AC1 {name}")).await;
        members.push((name, ns, key));
    }

    client_o
        .tools_call("host.group.create", json!({"name": "nightly"}))
        .await
        .expect("group.create");
    for (_, ns, _) in &members {
        client_o
            .tools_call("host.group.add", json!({"name": "nightly", "namespace": ns}))
            .await
            .expect("group.add");
    }

    let opened = extract_structured(
        &client_o
            .tools_call("host.channel.open", json!({"group": "nightly"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();
    assert_eq!(opened["group"], json!("nightly"), "{opened:?}");

    let (_, ns_a, key_a) = &members[0];
    let client_a = McpClient::with_bearer(&server.base_url, key_a);
    let posted = extract_structured(
        &client_a
            .tools_call(
                "host.channel.post",
                json!({"channel": channel_id, "body": "index rebuilt"}),
            )
            .await
            .expect("A posts"),
    );
    assert_eq!(posted["seq"], json!(1), "{posted:?}");

    for (name, _, key) in &members[1..] {
        let client = McpClient::with_bearer(&server.base_url, key);
        let read = extract_structured(
            &client
                .tools_call("host.channel.read", json!({"channel_id": channel_id}))
                .await
                .unwrap_or_else(|e| panic!("{name} reads: {} {}", e.code, e.message)),
        );
        let posts = read["posts"].as_array().expect("posts array");
        assert_eq!(posts.len(), 1, "{name}'s read: {read:?}");
        assert_eq!(posts[0]["seq"], json!(1), "{name}'s read: {read:?}");
        assert_eq!(posts[0]["from_address"], json!(ns_a), "{name}'s read: {read:?}");
        assert_eq!(posts[0]["body"], json!("index rebuilt"), "{name}'s read: {read:?}");
    }

    // O (the owner, not a `host.group.add`ed member) can read too --
    // requirement 2's own owner-always-authorized contract.
    let owner_read = extract_structured(
        &client_o
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("owner reads"),
    );
    assert_eq!(owner_read["posts"].as_array().expect("posts").len(), 1, "{owner_read:?}");
}
