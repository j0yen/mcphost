//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC6 (P0) -- Given two tenants paging the same channel, When one passes the
//! other's cursor, Then `cursor_invalid {reason: "signature"}`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_cursor_minted_for_one_tenant_is_refused_for_another() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_o, key_o) = signup(&server.base_url, "Pgtr AC6 Owner").await;
    let (ns_a, key_a) = signup(&server.base_url, "Pgtr AC6 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Pgtr AC6 B").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_o);
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    owner.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    for ns in [&ns_a, &ns_b] {
        owner.tools_call("host.group.add", json!({"name": "g", "namespace": ns})).await.expect("group.add");
    }
    let opened = extract_structured(&owner.tools_call("host.channel.open", json!({"group": "g"})).await.expect("open"));
    let channel = opened["channel_id"].as_str().expect("channel_id").to_string();
    for n in 1..=3 {
        owner
            .tools_call("host.channel.post", json!({"channel": channel, "body": format!("p{n}")}))
            .await
            .expect("post");
    }

    let a_page = extract_structured(
        &a.tools_call("host.channel.read", json!({"channel_id": channel, "limit": 1})).await.expect("A reads"),
    );
    let a_cursor = a_page["next_cursor"].as_str().expect("A's page has a next_cursor").to_string();

    // A can resume from its own cursor ...
    let a_next = extract_structured(
        &a.tools_call("host.channel.read", json!({"channel_id": channel, "cursor": a_cursor, "limit": 1}))
            .await
            .expect("A resumes"),
    );
    assert_eq!(a_next["posts"][0]["seq"], json!(2), "{a_next:?}");

    // ... B cannot.
    let err = b
        .tools_call("host.channel.read", json!({"channel_id": channel, "cursor": a_cursor}))
        .await
        .expect_err("B must not accept A's cursor");
    assert_eq!(err.error_code.as_deref(), Some("cursor_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("signature"), "{err:?}");
    assert_eq!(err.data["remedy"], json!("omit cursor to restart from the first page"), "{err:?}");
}
