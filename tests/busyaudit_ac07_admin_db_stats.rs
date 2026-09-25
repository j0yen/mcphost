//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC7 — Given `admin.db.stats`, When called, Then it returns counters
//! per role, pragmas per role, `wal_bytes`, and `page_count`, and the
//! call is recorded in `admin_audit`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn admin_db_stats_reports_counters_pragmas_and_size_and_is_audited() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let result = admin
        .tools_call("admin.db.stats", json!({}))
        .await
        .expect("admin.db.stats");
    let structured = extract_structured(&result);

    let roles = structured["roles"].as_array().expect("roles array");
    assert_eq!(roles.len(), mcphost::db::ALL_ROLES.len(), "{structured:?}");
    let role_names: Vec<&str> = roles.iter().map(|r| r["role"].as_str().unwrap()).collect();
    for expected in mcphost::db::ALL_ROLES {
        assert!(role_names.contains(&expected), "missing role {expected}: {role_names:?}");
    }
    for role in roles {
        assert_eq!(role["pragmas"]["journal_mode"], json!("wal"), "{role:?}");
        assert_eq!(role["pragmas"]["synchronous"], json!("normal"), "{role:?}");
        assert_eq!(role["pragmas"]["foreign_keys"], json!(1), "{role:?}");
        assert!(role["pragmas"]["busy_timeout"].as_i64().is_some(), "{role:?}");
        assert!(role["busy_total"].as_i64().is_some(), "{role:?}");
        assert!(role["locked_total"].as_i64().is_some(), "{role:?}");
        assert!(role["wait_gt100ms_total"].as_i64().is_some(), "{role:?}");
        assert!(role["wait_max_ms"].as_i64().is_some(), "{role:?}");
    }
    assert!(structured["wal_bytes"].as_i64().is_some(), "{structured:?}");
    assert!(structured["page_count"].as_i64().unwrap() > 0, "{structured:?}");

    // requirement 4 / AC7: recorded in admin_audit even though read-only.
    let audit_result = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log");
    let audit = extract_structured(&audit_result);
    let entries = audit["entries"].as_array().expect("entries array");
    assert!(
        entries.iter().any(|e| e["action"] == json!("db_stats")),
        "admin.db.stats must leave an admin_audit row: {entries:?}"
    );
}
