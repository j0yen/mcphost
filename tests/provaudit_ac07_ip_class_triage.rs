//! PRD-mcphost-provenance-audit
//! AC7 (P1) — Given three signups from loopback/private/public IPs, When
//! queried, Then `origin_detail` -- via `signup_events.ip_class` --
//! carries the correct ip class for each.

use crate::common;
use common::TestServer;
use rusqlite::params;
use serde_json::json;

/// No existing `Db` accessor reads back a `signup_events` row, so this
/// test opens a second raw connection against the same sqlite file
/// `Db::open` created -- same pattern `tenant_delete_ac08`/`provaudit_ac06`
/// use for direct schema-level assertions.
fn ip_class_for(db_path: &std::path::Path, source_ip: &str) -> String {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT ip_class FROM signup_events WHERE source_ip = ?1 ORDER BY id DESC LIMIT 1",
        params![source_ip],
        |r| r.get(0),
    )
    .expect("query signup_events.ip_class")
}

#[tokio::test]
async fn signup_events_ip_class_triages_loopback_private_public() {
    let server = TestServer::start().await;
    let db_path = server.data_dir.0.join("mcphost.db");

    for source_ip in ["127.0.0.1", "10.1.2.3", "203.0.113.7"] {
        mcphost::control::signup(
            &server.state,
            &json!({"name": format!("IP Class Probe {source_ip}")}),
            source_ip,
            mcphost::control::SignupAttribution::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("signup from {source_ip} failed: {e:?}"));
    }

    assert_eq!(ip_class_for(&db_path, "127.0.0.1"), "loopback");
    assert_eq!(ip_class_for(&db_path, "10.1.2.3"), "private");
    assert_eq!(ip_class_for(&db_path, "203.0.113.7"), "public");
}
