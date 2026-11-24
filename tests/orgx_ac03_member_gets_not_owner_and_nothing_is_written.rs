//! PRD-mcphost-org-export-and-offboard-v2
//! AC3 — Given a member caller, When it calls `host.org.export`,
//! `host.org.offboard` or `host.org.restore`, Then it gets `not_owner` and
//! nothing is written.

use crate::common;
use common::TestServer;
use common::orgx::fixture_org;
use serde_json::json;

/// Row counts of every table a verb of this PRD could write to, plus the
/// disabled-tenant count and the exports directory listing.
fn snapshot(server: &TestServer) -> (i64, i64, i64, i64, Vec<String>) {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).expect("count");
    let exports = std::fs::read_dir(server.state.db.data_dir().join("exports"))
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    (
        count("SELECT COUNT(*) FROM runs"),
        count("SELECT COUNT(*) FROM tenant_audit"),
        count("SELECT COUNT(*) FROM tenants WHERE disabled = 1"),
        count("SELECT COUNT(*) FROM org_members WHERE removed_unix IS NULL"),
        exports,
    )
}

#[tokio::test]
async fn orgx_ac03() {
    let server = TestServer::start().await;
    let fx = fixture_org(&server, "acme", 2, 1).await;
    let (_, member) = &fx.members[0];
    let before = snapshot(&server);

    for (verb, args) in [
        ("host.org.export", json!({})),
        ("host.org.offboard", json!({"confirm": "acme"})),
        ("host.org.restore", json!({})),
    ] {
        let err = member.tools_call(verb, args).await.expect_err(verb);
        assert_eq!(err.error_code.as_deref(), Some("not_owner"), "{verb}: {err:?}");
    }

    assert_eq!(snapshot(&server), before, "a refused member call writes nothing");
    // The member and the owner are still enabled: the member's next call answers.
    member.tools_call("host.whoami", json!({})).await.expect("member still enabled");
    fx.owner.tools_call("host.whoami", json!({})).await.expect("owner still enabled");
}
