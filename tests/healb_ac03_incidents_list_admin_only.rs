//! PRD-mcphost-heal-action-registry-and-ledger-b AC3 (P1): `host.incidents.list`
//! is admin-only: hidden from anonymous `tools/list`, `AppError::Unauthorized` when an
//! anonymous caller invokes it, and for the admin returns raw `trigger_json`
//! with `limit` clamped to 200.

use crate::common::{self, McpClient, TestServer};

use mcphost::heal::{HealAction, HealOutcome, HealRow};
use serde_json::json;

#[tokio::test]
async fn healb_ac03_incidents_list_is_admin_only_and_clamps_limit() {
    let server = TestServer::start().await;
    let trigger = json!({"reason": "wal", "tenant_key": "kept-raw"});
    for i in 0..3 {
        let row = HealRow {
            id: 0,
            action: HealAction::DbMaintenance,
            at: mcphost::state::now_unix() - i,
            outcome: HealOutcome::Ok,
            dry_run: false,
            duration_ms: 5,
            trigger: trigger.clone(),
            before: json!({"wal_mb": 5.7}),
            after: json!({"wal_mb": 0}),
        };
        server.state.db.insert_heal_row(row).await.expect("insert");
    }

    let anon = McpClient::new(&server.base_url);
    let listed = anon.tools_list().await.expect("anon tools/list");
    assert!(
        !listed.to_string().contains("host.incidents.list"),
        "anonymous tools/list must not carry host.incidents.list"
    );
    let err = anon.tools_call("host.incidents.list", json!({})).await.expect_err("anonymous call refused");
    // `AppError::Unauthorized`'s wire code is `bearer_invalid` (errors.rs: renamed
    // from "unauthorized"); never `tenant_key_missing` (no implicit signup).
    assert_eq!(err.error_code.as_deref(), Some("bearer_invalid"), "{err:?}");
    assert!(err.message.contains("Authorization"), "{err:?}");

    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let out = admin
        .tools_call("host.incidents.list", json!({"limit": 100000}))
        .await
        .expect("admin call");
    let body = common::extract_structured(&out);
    assert_eq!(body["limit"], json!(200));
    let rows = body["incidents"].as_array().expect("incidents");
    assert_eq!(rows.len(), 3);
    let raw: serde_json::Value = serde_json::from_str(rows[0]["trigger_json"].as_str().expect("trigger_json")).unwrap();
    assert_eq!(raw, trigger, "raw trigger_json intact, not anonymised");

    let one = admin.tools_call("host.incidents.list", json!({"limit": 1})).await.expect("limit 1");
    assert_eq!(common::extract_structured(&one)["incidents"].as_array().map(Vec::len), Some(1));
}
