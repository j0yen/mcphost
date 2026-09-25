//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC8 — Given the operator healthz header, When `GET /healthz` is
//! called, Then `db.busy_total`, `db.locked_total`, and `db.wait_max_ms`
//! are present; the anonymous healthz body is unchanged.

use crate::common;
use common::{ADMIN_KEY, TestServer};

#[tokio::test]
async fn operator_healthz_carries_db_counters_anonymous_body_unchanged() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let anon: serde_json::Value = http
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("anonymous healthz")
        .json()
        .await
        .expect("anonymous healthz json");
    assert_eq!(
        anon,
        serde_json::json!({"ok": true}),
        "anonymous healthz body must be unchanged: {anon:?}"
    );

    let operator: serde_json::Value = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("operator healthz")
        .json()
        .await
        .expect("operator healthz json");
    assert!(
        operator["db"]["busy_total"].as_i64().is_some(),
        "{operator:?}"
    );
    assert!(
        operator["db"]["locked_total"].as_i64().is_some(),
        "{operator:?}"
    );
    assert!(
        operator["db"]["wait_max_ms"].as_i64().is_some(),
        "{operator:?}"
    );
}
