//! PRD-mcphost-abuse-guard-ban-list
//! AC5 (P0) — Given an address with 30 `claim_rate_events` in 10 min, When
//! the tick runs, Then an auto ban exists for `addr` and
//! `/claim/verify/<code>` from that address returns `banned`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn thirty_claim_rate_events_in_ten_minutes_auto_bans_the_address_then_verify_is_refused() {
    let server = TestServer::start().await;
    let addr = "203.0.113.30";
    let since = mcphost::state::now_unix() - 600;

    for _ in 0..30 {
        server
            .state
            .db
            .try_admit_claim_request(addr.to_string(), since, 1000)
            .await
            .expect("try_admit_claim_request");
    }

    mcphost::bans::tick_once(&server.state).await.expect("tick_once");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let list = admin
        .tools_call("admin.ban.list", json!({"active_only": true, "subject_kind": "addr"}))
        .await
        .expect("admin.ban.list");
    let rows = extract_structured(&list)["bans"].as_array().cloned().unwrap_or_default();
    rows.iter()
        .find(|r| r["subject"] == json!(addr) && r["auto"] == json!(true))
        .unwrap_or_else(|| panic!("no auto-ban for {addr} in {rows:?}"));

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/claim/verify/nonexistent-code", server.base_url))
        .header("x-forwarded-for", addr)
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
    let body = resp.text().await.expect("body");
    assert!(
        body.to_lowercase().contains("banned"),
        "expected the banned page, got: {body}"
    );
}
