//! PRD-mcphost-healthz-version-field AC2 — Given the database is unwritable,
//! When an anonymous client GETs `/healthz`, Then the status is 503 and the
//! body still carries `version` and `git_sha`.

use crate::common;
use common::TestServer;
use mcphost::build_info::BUILD;
use serde_json::json;

#[tokio::test]
async fn unwritable_db_503_body_still_names_the_build() {
    let server = TestServer::start().await;
    server
        .state
        .db
        .set_query_only(true)
        .await
        .expect("simulate unwritable db");

    let resp = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(body["git_sha"], json!(BUILD.git_sha));
    assert_eq!(body.as_object().unwrap().len(), 3, "body: {body}");
}
