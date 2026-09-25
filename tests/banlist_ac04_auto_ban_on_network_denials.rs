//! PRD-mcphost-abuse-guard-ban-list
//! AC4 (P0) — Given a tenant key with 25 `network_denials` rows in the
//! last 10 min, When the minute tick runs, Then a ban row with `auto=true`
//! and a 1 h expiry exists for that key. (The alert half of this AC --
//! "if the alerting registry is present, one `ban.applied` alert is
//! raised" -- has no alerting registry to exercise in this crate today;
//! see `bans.rs`'s module doc.)

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn twenty_five_network_denials_in_ten_minutes_auto_bans_the_key() {
    let server = TestServer::start().await;
    let (_tenant_ns, key) = signup(&server.base_url, "Denial Flooder").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(&key))
        .await
        .expect("find tenant")
        .expect("tenant exists");

    for _ in 0..25 {
        server
            .state
            .db
            .record_network_denial("run_plan", Some(tenant.id))
            .await
            .expect("record_network_denial");
    }

    mcphost::bans::tick_once(&server.state).await.expect("tick_once");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let list = admin
        .tools_call("admin.ban.list", json!({"active_only": true, "subject_kind": "key"}))
        .await
        .expect("admin.ban.list");
    let rows = extract_structured(&list)["bans"].as_array().cloned().unwrap_or_default();
    let row = rows
        .iter()
        .find(|r| r["subject"] == json!(tenant.key_hash))
        .unwrap_or_else(|| panic!("no auto-ban for the flooding tenant's key in {rows:?}"));
    assert_eq!(row["auto"].as_bool(), Some(true));

    let now = mcphost::state::now_unix();
    let expires_at = row["expires_at"].as_i64().expect("expires_at present");
    assert!(
        (now + 3500..=now + 3700).contains(&expires_at),
        "expected roughly a 1h expiry, got {expires_at} (now={now})"
    );

    // A key that never crossed the threshold is unaffected.
    let (_clean_ns, clean_key) = signup(&server.base_url, "Clean Tenant").await;
    let clean_client = McpClient::with_bearer(&server.base_url, &clean_key);
    clean_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("an unbanned tenant's call must still succeed after the tick");
}
