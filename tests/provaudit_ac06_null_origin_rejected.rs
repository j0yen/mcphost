//! PRD-mcphost-provenance-audit
//! AC6 — Given an attempted write path that would leave `origin` NULL,
//! When exercised in tests, Then the write is rejected (constraint), not
//! defaulted silently.

mod common;
use common::{TestServer, signup};
use rusqlite::params;

#[tokio::test]
async fn calls_origin_not_null_constraint_rejects_an_explicit_null() {
    let server = TestServer::start().await;
    let (tenant_ns, _key) = signup(&server.base_url, "Seeded Real Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .expect("query")
        .expect("tenant exists");

    // A second, independent raw connection against the same sqlite file
    // `Db::open` created -- proving the schema itself rejects a NULL
    // `origin`, not just this crate's own write paths (which never attempt
    // one).
    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let result = conn.execute(
        "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, origin) \
         VALUES (?1, 'nulled_tool', 'unix:0.0', 0, 1, 1, NULL)",
        params![tenant.id],
    );
    assert!(
        result.is_err(),
        "an explicit NULL origin must violate the NOT NULL constraint, not insert silently"
    );
}
