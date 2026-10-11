//! PRD-mcphost-healthz-anon-keys-const AC5 (P0) -- an anonymous
//! `GET /healthz` on a running server whose database is unwritable returns
//! exactly `ANON_HEALTHZ_KEYS` with `ok: false` (503), the same key set as
//! the healthy body (the `ac14_storage_unwritable` path).

use crate::common;
use common::TestServer;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn unwritable_database_anonymous_healthz_keys_equal_the_const() {
    let server = TestServer::start().await;
    server.state.db.set_query_only(true).await.expect("query_only on");

    let resp = reqwest::get(format!("{}/healthz", server.base_url)).await.expect("GET /healthz");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    let mut keys: Vec<&str> = body.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, common::anon_healthz_keys(), "ANON_HEALTHZ_KEYS vs unwritable body: {body}");
    assert_eq!(body["ok"], json!(false), "body: {body}");

    server.state.db.set_query_only(false).await.expect("query_only off");
}
