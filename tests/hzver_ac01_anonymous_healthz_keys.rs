//! PRD-mcphost-healthz-version-field AC1 — Given a build of the server, When
//! an anonymous client GETs `/healthz`, Then the body is exactly the keys
//! `ok`, `version`, `git_sha` with `version` equal to `CARGO_PKG_VERSION`.

use crate::common;
use common::TestServer;

#[tokio::test]
async fn anonymous_healthz_has_exactly_ok_version_git_sha() {
    let server = TestServer::start().await;

    let resp = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("parse /healthz");

    let mut keys: Vec<&str> = body.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["git_sha", "ok", "version"], "body: {body}");
    assert_eq!(body["ok"], serde_json::json!(true));
    assert_eq!(body["version"], serde_json::json!(env!("CARGO_PKG_VERSION")));
    assert!(body["git_sha"].is_string() || body["git_sha"].is_null());
}
