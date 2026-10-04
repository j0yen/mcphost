//! PRD-mcphost-implicit-signup
//! AC4 (P0) — Given the signup pause file is present, When an anonymous
//! `host.*` call arrives, Then `signup_paused` is returned within 1 s and
//! nothing is created.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn paused_signup_refuses_a_bare_call_fast_and_creates_nothing() {
    let server = TestServer::start().await;

    std::fs::write(
        server.state.signup_pause.path(),
        "maintenance window, back in five minutes\n",
    )
    .expect("write pause file");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let session = McpClient::new(&server.base_url).with_session_continuity();
    let started = Instant::now();
    let err = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a bare host.* call while paused must be refused");
    let elapsed = started.elapsed();

    assert_eq!(err.error_code.as_deref(), Some("signup_paused"), "{err:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "signup_paused must be returned within 1s, took {elapsed:?}"
    );

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 0, "nothing must be created while paused");

    // Resume: the pause file is removed, and a bare call succeeds again.
    std::fs::remove_file(server.state.signup_pause.path()).expect("remove pause file");
    tokio::time::sleep(Duration::from_secs(1)).await;
    session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("a bare host.* call after resume must succeed");
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1);
}
