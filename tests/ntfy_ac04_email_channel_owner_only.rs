//! PRD-mcphost-notify-user-channel
//! AC4 -- Given a tenant whose `owner_verified_at` is null, When
//! `host.notify.channel_add {kind:"email", target: <owner_email>}` runs, Then
//! it fails `owner_unverified` with `next_call: host.claim`; Given a verified
//! owner, Then the same call succeeds and a different address fails
//! `email_not_owner`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const OWNER: &str = "owner@example.com";

fn set_owner(server: &TestServer, namespace: &str, verified_at: Option<i64>) {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let changed = conn
        .execute(
            "UPDATE tenants SET owner_email = ?1, owner_verified_at = ?2 WHERE namespace = ?3",
            rusqlite::params![OWNER, verified_at, namespace],
        )
        .expect("set owner");
    assert_eq!(changed, 1, "tenant row for {namespace}");
}

#[tokio::test]
async fn ntfy_ac04() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "ntfy ac04 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);

    // The owner entered an address but never verified it.
    set_owner(&server, &ns, None);
    let refused = agent
        .tools_call("host.notify.channel_add", json!({"kind": "email", "target": OWNER}))
        .await
        .expect_err("unverified owner");
    assert_eq!(refused.error_code.as_deref(), Some("owner_unverified"), "{refused:?}");
    assert_eq!(refused.data["next_call"], json!("host.claim"), "{refused:?}");

    // Verified: the owner's own address is accepted, any other is not.
    set_owner(&server, &ns, Some(1_700_000_000));
    let added = extract_structured(
        &agent
            .tools_call("host.notify.channel_add", json!({"kind": "email", "target": OWNER}))
            .await
            .expect("verified owner's address"),
    );
    assert_eq!(added["kind"], json!("email"), "{added}");
    assert!(added["channel_id"].is_string(), "{added}");

    let other = agent
        .tools_call("host.notify.channel_add", json!({"kind": "email", "target": "someone.else@example.com"}))
        .await
        .expect_err("a different address");
    assert_eq!(other.error_code.as_deref(), Some("email_not_owner"), "{other:?}");

    let listed = extract_structured(&agent.tools_call("host.notify.channel_list", json!({})).await.expect("list"));
    assert_eq!(listed["channels"].as_array().map(Vec::len), Some(1), "{listed}");
}
