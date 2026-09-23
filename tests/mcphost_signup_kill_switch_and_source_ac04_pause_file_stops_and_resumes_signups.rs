//! AC4 (PRD-mcphost-signup-kill-switch-and-source) — Given the pause file
//! is created while the server runs, When `signup` is called 1s later,
//! Then the error code is `signup_paused`, `message` equals the file's
//! first line, and `retry_after_secs` is present; When the file is
//! removed, Then a signup 1s later succeeds. No restart in between.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn pause_file_stops_then_resumes_signups_with_no_restart() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    client
        .tools_call("signup", json!({"name": "Before Pause"}))
        .await
        .expect("signup before pause must succeed");

    std::fs::write(
        server.state.signup_pause.path(),
        "maintenance window, back in five minutes\n",
    )
    .expect("write pause file");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let err = client
        .tools_call("signup", json!({"name": "During Pause"}))
        .await
        .expect_err("signup while paused must fail");
    assert_eq!(err.error_code.as_deref(), Some("signup_paused"), "{err:?}");
    assert_eq!(err.message, "maintenance window, back in five minutes", "{err:?}");
    assert!(
        err.data["retry_after_secs"].is_i64() || err.data["retry_after_secs"].is_u64(),
        "retry_after_secs must be present, got {err:?}"
    );

    std::fs::remove_file(server.state.signup_pause.path()).expect("remove pause file");
    tokio::time::sleep(Duration::from_secs(1)).await;

    client
        .tools_call("signup", json!({"name": "After Resume"}))
        .await
        .expect("signup after resume must succeed, no restart");
}
