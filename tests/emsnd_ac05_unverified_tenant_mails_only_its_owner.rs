//! PRD-mcphost-send-email-from-tool
//! AC5 (P0) -- Given a tenant without `owner_verified_at`, When it sends to
//! any address but its `owner_email`, Then the error is
//! `email_owner_unverified` and nothing is sent; a send to the owner succeeds.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::FakeEmailClient;
use serde_json::json;
use std::sync::Arc;

const OWNER: &str = "owner@example.com";

#[tokio::test]
async fn emsnd_ac05_only_the_owner_is_reachable_until_verified() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "emsnd ac05 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    // The owner entered an address but never verified it.
    conn.execute(
        "UPDATE tenants SET owner_email = ?1, owner_verified_at = NULL WHERE namespace = ?2",
        rusqlite::params![OWNER, ns],
    )
    .expect("set owner");

    let refused = agent
        .tools_call("host.email.send", json!({"to": "stranger@example.org", "subject": "hi", "text": "body"}))
        .await
        .expect_err("a stranger before verification");
    assert_eq!(refused.error_code.as_deref(), Some("email_owner_unverified"), "{refused:?}");
    assert_eq!(refused.data["next_call"], json!("host.claim"), "{refused:?}");
    assert_eq!(fake.send_count(), 0, "nothing is sent");

    let mixed = agent
        .tools_call("host.email.send", json!({"to": [OWNER, "stranger@example.org"], "subject": "hi", "text": "body"}))
        .await
        .expect_err("one stranger among the recipients refuses the whole call");
    assert_eq!(mixed.error_code.as_deref(), Some("email_owner_unverified"), "{mixed:?}");
    assert_eq!(fake.send_count(), 0, "nothing is sent");

    let sent = extract_structured(
        &agent
            .tools_call("host.email.send", json!({"to": OWNER.to_uppercase(), "subject": "hi", "text": "body"}))
            .await
            .expect("the owner is always reachable"),
    );
    assert!(sent["id"].is_string(), "{sent}");
    assert_eq!(fake.send_count(), 1);

    // Once verified, any address is reachable.
    conn.execute("UPDATE tenants SET owner_verified_at = 1700000000 WHERE namespace = ?1", rusqlite::params![ns])
        .expect("verify owner");
    agent
        .tools_call("host.email.send", json!({"to": "stranger@example.org", "subject": "hi", "text": "body"}))
        .await
        .expect("a verified tenant may mail anyone");
    assert_eq!(fake.send_count(), 2);

    let listed = extract_structured(&agent.tools_call("host.email.sends", json!({"limit": 10})).await.expect("sends"));
    let statuses: Vec<&str> = listed["sends"].as_array().expect("array").iter().filter_map(|r| r["status"].as_str()).collect();
    assert_eq!(statuses.iter().filter(|s| **s == "owner_unverified").count(), 2, "{listed}");
}
