//! PRD-mcphost-send-email-from-tool
//! AC4 (P0) -- Given a free-plan tenant with 50 sends today, When the 51st is
//! attempted, Then the error is `email_quota_exceeded` with `data.used == 50`,
//! `data.limit == 50`, `data.resets_at`, no provider call and a row with
//! status `quota_exceeded`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::FakeEmailClient;
use mcphost::plans::PlanCatalog;
use serde_json::json;
use std::sync::Arc;

const OWNER: &str = "owner@example.com";

#[tokio::test]
async fn emsnd_ac04_the_51st_send_is_refused_with_the_quota_remedy() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "emsnd ac04 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.execute(
        "UPDATE tenants SET owner_email = ?1, owner_verified_at = 1700000000 WHERE namespace = ?2",
        rusqlite::params![OWNER, ns],
    )
    .expect("verify owner");
    drop(conn);

    let limit = PlanCatalog::default_catalog().get("free").expect("free plan").emails_per_day;
    assert_eq!(limit, 50, "the free plan's emails_per_day is 50");
    let quickstart = extract_structured(&agent.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["plan"]["emails_per_day"], json!(50), "{quickstart}");

    for n in 0..50 {
        agent
            .tools_call("host.email.send", json!({"to": OWNER, "subject": format!("mail {n}"), "text": "body"}))
            .await
            .unwrap_or_else(|e| panic!("send {n}: {e:?}"));
    }
    assert_eq!(fake.send_count(), 50);

    let err = agent
        .tools_call("host.email.send", json!({"to": OWNER, "subject": "mail 50", "text": "body"}))
        .await
        .expect_err("the 51st send must be refused");
    assert_eq!(err.error_code.as_deref(), Some("email_quota_exceeded"), "{err:?}");
    assert_eq!(err.data["used"], json!(50), "{err:?}");
    assert_eq!(err.data["limit"], json!(50), "{err:?}");
    assert!(err.data["resets_at"].is_string(), "{err:?}");
    assert_eq!(fake.send_count(), 50, "no provider call for the refused send");

    let listed = extract_structured(&agent.tools_call("host.email.sends", json!({"limit": 1})).await.expect("sends"));
    assert_eq!(listed["sends"][0]["status"], json!("quota_exceeded"), "{listed}");
}
