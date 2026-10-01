//! PRD-mcphost-implicit-signup
//! AC4 (P0) — Given the signup pause file is present, When an anonymous
//! `host.*` call arrives, Then `signup_paused` is returned within 1 s and
//! nothing is created.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn pause_file_refuses_implicit_signup_within_1s() {
    let server = TestServer::start().await;

    std::fs::write(server.state.signup_pause.path(), "paused for maintenance\n")
        .expect("write pause file");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let before = server.state.db.list_tenants().await.unwrap().len();
    let client = McpClient::new(&server.base_url);

    let started = std::time::Instant::now();
    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a bare host.* call while paused must be refused, not implicitly signed up");
    let elapsed = started.elapsed();

    assert_eq!(err.error_code.as_deref(), Some("signup_paused"), "{err:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "must refuse within 1s, took {elapsed:?}"
    );

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after, before,
        "nothing must be created while signups are paused"
    );

    std::fs::remove_file(server.state.signup_pause.path()).expect("remove pause file");
}
