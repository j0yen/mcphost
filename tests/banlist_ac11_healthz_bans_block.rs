//! PRD-mcphost-abuse-guard-ban-list
//! AC11 (P1) — Given the operator healthz header, When `GET /healthz` is
//! called, Then `bans.active`, `bans.auto_active`, and `bans.hits_24h` are
//! present.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn healthz_reports_active_auto_active_and_hits_24h() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let anon = McpClient::new(&server.base_url);

    let before = healthz(&server.base_url).await;
    assert_eq!(before["bans"]["active"], json!(0));
    assert_eq!(before["bans"]["auto_active"], json!(0));
    assert_eq!(before["bans"]["hits_24h"], json!(0));

    admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.70", "ttl": "24h", "reason": "flood"}),
        )
        .await
        .expect("admin.ban.add");
    mcphost::bans::tick_once(&server.state).await.expect("tick_once with nothing over threshold");

    let _ = anon
        .tools_call_with_header(
            "signup",
            json!({"name": "Hit The Ban"}),
            ("x-forwarded-for", "203.0.113.70"),
        )
        .await;

    let after = healthz(&server.base_url).await;
    assert_eq!(after["bans"]["active"], json!(1), "the one operator ban: {after}");
    assert_eq!(
        after["bans"]["auto_active"], json!(0),
        "no auto-ban should have fired with no denials/claim-rate flood: {after}"
    );
    assert_eq!(after["bans"]["hits_24h"], json!(1), "the one refused signup above: {after}");
}
