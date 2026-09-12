//! PRD-mcphost-healthz-minimal
//! AC4 — Given a failed liveness condition (e.g. db unavailable in test),
//! When anonymous `/healthz` is hit, Then it returns `{"ok": false}` with a
//! 5xx status and still no diagnostic fields.
//!
//! "Unavailable" is simulated the same way `tests/ac14_storage_unwritable.rs`
//! does: `PRAGMA query_only = ON` via `Db::set_query_only`, SQLite's own
//! first-class way to make writes fail without an unreliable `chmod` on an
//! already-open fd.

use crate::common;
use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn anonymous_healthz_reports_ok_false_and_5xx_on_liveness_failure() {
    let server = TestServer::start().await;
    server
        .state
        .db
        .set_query_only(true)
        .await
        .expect("simulate unwritable db (liveness failure)");

    let resp = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz");
    assert!(
        resp.status().is_server_error(),
        "expected a 5xx status on liveness failure, got {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    assert_eq!(
        body,
        json!({"ok": false}),
        "anonymous body must stay minimal even on a liveness failure: {body:?}"
    );
}
