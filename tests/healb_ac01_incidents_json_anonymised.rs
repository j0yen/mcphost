//! PRD-mcphost-heal-action-registry-and-ledger-b AC1 (P0): `/incidents.json`
//! drops tenant identifiers from `before`, keeps allowlisted keys and is
//! cacheable for 30 s.

use crate::common;

use mcphost::heal::{HealAction, HealOutcome, HealRow};
use serde_json::{Value, json};

#[tokio::test]
async fn healb_ac01_incidents_json_drops_tenant_keys_and_sets_cache_control() {
    let server = common::TestServer::start().await;
    let row = HealRow {
        id: 0,
        action: HealAction::DbMaintenance,
        at: mcphost::state::now_unix(),
        outcome: HealOutcome::Ok,
        dry_run: false,
        duration_ms: 12,
        trigger: json!({"reason": "wal", "tenant_key": "k"}),
        before: json!({"tenant_id": 42, "tool_name": "t42/secret_tool", "wal_mb": 5.7, "count": 3}),
        after: json!({"wal_mb": 0}),
    };
    server.state.db.insert_heal_row(row).await.expect("insert");

    let resp = reqwest::get(format!("{}/incidents.json", server.base_url)).await.expect("get");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("cache-control").and_then(|v| v.to_str().ok()), Some("max-age=30"));
    let body: Value = resp.json().await.expect("json");
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    let before = rows[0]["before"].as_object().expect("before");
    assert!(!before.contains_key("tenant_id"));
    assert!(!before.contains_key("tool_name"));
    assert_eq!(before["wal_mb"], json!(5.7));
    assert_eq!(before["count"], json!(3));
    assert!(rows[0]["trigger"].get("tenant_key").is_none());
    assert_eq!(rows[0]["trigger"]["reason"], json!("wal"));
    assert_eq!(rows[0]["action"], json!("db_maintenance"));
    assert!(!body.to_string().contains("t42/"));
}
