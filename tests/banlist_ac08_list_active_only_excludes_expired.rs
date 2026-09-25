//! PRD-mcphost-abuse-guard-ban-list
//! AC8 (P0) — Given `admin.ban.list {active_only: true}`, When called
//! after two active and one expired ban, Then two rows return with
//! `hits`, `reason`, `expires_at`, `auto`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn active_only_returns_exactly_the_two_unexpired_bans() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.50", "ttl": "24h", "reason": "one"}),
        )
        .await
        .expect("ban 1");
    admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.51", "ttl": "24h", "reason": "two"}),
        )
        .await
        .expect("ban 2");
    let expired = admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.52", "ttl": "24h", "reason": "three"}),
        )
        .await
        .expect("ban 3");
    let expired_id = extract_structured(&expired)["id"].as_i64().expect("id");
    server
        .state
        .db
        .set_ban_expires_at_for_test(expired_id, mcphost::state::now_unix() - 10)
        .await
        .expect("force expiry");

    let list = admin
        .tools_call("admin.ban.list", json!({"active_only": true}))
        .await
        .expect("admin.ban.list");
    let rows = extract_structured(&list)["bans"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 2, "expired ban must be excluded: {rows:?}");
    for row in &rows {
        assert_ne!(row["id"].as_i64(), Some(expired_id));
        assert!(row.get("hits").is_some());
        assert!(row.get("reason").is_some());
        assert!(row.get("expires_at").is_some());
        assert!(row.get("auto").is_some());
    }

    let unfiltered = admin
        .tools_call("admin.ban.list", json!({"active_only": false}))
        .await
        .expect("admin.ban.list unfiltered");
    let all_rows = extract_structured(&unfiltered)["bans"].as_array().cloned().unwrap_or_default();
    assert_eq!(all_rows.len(), 3, "unfiltered list must still include the expired row: {all_rows:?}");
}
