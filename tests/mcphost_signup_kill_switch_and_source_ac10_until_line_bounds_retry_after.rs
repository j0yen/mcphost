//! AC10 (PRD-mcphost-signup-kill-switch-and-source) — Given a pause file
//! whose second line is `until=<now+120>`, When signup is called, Then
//! `retry_after_secs` is between 100 and 120.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn until_line_bounds_retry_after_secs() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let until = mcphost::state::now_unix() + 120;
    std::fs::write(
        server.state.signup_pause.path(),
        format!("back soon\nuntil={until}\n"),
    )
    .expect("write pause file");

    let err = client
        .tools_call("signup", json!({"name": "Ac10 Agent"}))
        .await
        .expect_err("signup while paused must fail");
    assert_eq!(err.error_code.as_deref(), Some("signup_paused"), "{err:?}");
    let retry_after_secs = err.data["retry_after_secs"]
        .as_i64()
        .expect("retry_after_secs must be an integer");
    assert!(
        (100..=120).contains(&retry_after_secs),
        "retry_after_secs must be between 100 and 120, got {retry_after_secs}"
    );
}
