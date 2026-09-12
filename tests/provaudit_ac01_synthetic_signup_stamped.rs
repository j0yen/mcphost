//! PRD-mcphost-provenance-audit
//! AC1 — Given a signup carrying the synthetic marker (header or
//! key-class), When it lands, Then its tenant and signup_event rows carry
//! `origin = synthetic` with the run-id in `origin_detail` (not just the
//! literal word "synthetic").

use crate::common;
use common::TestServer;
use rusqlite::params;
use serde_json::json;

#[tokio::test]
async fn synthetic_marker_signup_stamps_origin_synthetic_with_run_id_detail() {
    let server = TestServer::start().await;

    // A non-loopback source IP, but carrying the `x-mcphost-synthetic`
    // marker header (via `control::signup`'s attribution struct directly,
    // the same way `attrib_ac4`/`synthetic_ac07` exercise a specific
    // source IP without a real TCP connection) -- proves the marker alone
    // drives the synthetic verdict, independent of IP.
    let run_id = "synthorg:run-9001";
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "Stress QA Persona"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution {
            synthetic_header: Some(run_id),
            ..Default::default()
        },
    )
    .await
    .expect("synthetic-marked signup");
    let ns = result["tenant"].as_str().expect("tenant field").to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.origin, "synthetic");
    assert_eq!(
        tenant.origin_detail.as_deref(),
        Some(run_id),
        "origin_detail must carry the actual run-id, not just 'synthetic'"
    );

    // The durable `signup_events` ledger row carries the same stamp.
    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let (origin, origin_detail): (String, Option<String>) = conn
        .query_row(
            "SELECT origin, origin_detail FROM signup_events WHERE source_ip = ?1 \
             ORDER BY id DESC LIMIT 1",
            params!["8.8.8.8"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("query signup_events row");
    assert_eq!(origin, "synthetic");
    assert_eq!(origin_detail.as_deref(), Some(run_id));
}
