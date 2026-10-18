//! PRD-mcphost-send-email-from-tool
//! AC6 (P0) -- Given `to` with 11 addresses or one malformed address, When
//! called, Then `args_invalid` names the problem and no row is written;
//! Given the provider returns 503 twice, Then the row reads `provider_error`
//! and the caller gets `email_provider_error {status: 503}`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::FakeEmailClient;
use serde_json::json;
use std::sync::Arc;

const OWNER: &str = "owner@example.com";

async fn rows(agent: &McpClient) -> Vec<serde_json::Value> {
    let listed = extract_structured(&agent.tools_call("host.email.sends", json!({"limit": 100})).await.expect("sends"));
    listed["sends"].as_array().expect("array").clone()
}

#[tokio::test]
async fn emsnd_ac06_bad_recipients_write_no_row_and_two_503s_are_a_provider_error() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "emsnd ac06 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.execute(
        "UPDATE tenants SET owner_email = ?1, owner_verified_at = 1700000000 WHERE namespace = ?2",
        rusqlite::params![OWNER, ns],
    )
    .expect("verify owner");
    drop(conn);

    let eleven: Vec<String> = (0..11).map(|n| format!("user{n}@example.com")).collect();
    let too_many = agent
        .tools_call("host.email.send", json!({"to": eleven, "subject": "s", "text": "t"}))
        .await
        .expect_err("11 addresses");
    assert_eq!(too_many.error_code.as_deref(), Some("args_invalid"), "{too_many:?}");
    assert!(too_many.message.contains("10"), "names the limit: {too_many:?}");

    let malformed = agent
        .tools_call("host.email.send", json!({"to": ["ok@example.com", "not-an-address"], "subject": "s", "text": "t"}))
        .await
        .expect_err("malformed address");
    assert_eq!(malformed.error_code.as_deref(), Some("args_invalid"), "{malformed:?}");
    assert!(malformed.message.contains("not-an-address"), "names the address: {malformed:?}");

    assert!(rows(&agent).await.is_empty(), "no row for an argument failure");
    assert_eq!(fake.send_count(), 0);

    // The provider answers 503 to the send and to its one retry.
    fake.fail_next_with_status(2, 503);
    let failed = agent
        .tools_call("host.email.send", json!({"to": OWNER, "subject": "s", "text": "t"}))
        .await
        .expect_err("503 twice");
    assert_eq!(failed.error_code.as_deref(), Some("email_provider_error"), "{failed:?}");
    assert_eq!(failed.data["status"], json!(503), "{failed:?}");
    let listed = rows(&agent).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["status"], json!("provider_error"), "{listed:?}");
    assert_eq!(fake.send_count(), 0);

    // One 503 is absorbed by the retry.
    fake.fail_next_with_status(1, 503);
    let ok = extract_structured(
        &agent
            .tools_call("host.email.send", json!({"to": OWNER, "subject": "s", "text": "t"}))
            .await
            .expect("the retry succeeds"),
    );
    assert!(ok["id"].is_string(), "{ok}");
    assert_eq!(fake.send_count(), 1);
}
