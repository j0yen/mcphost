//! AC2 (PRD-mcphost-admin-signup-pause-tool) — Given no pause file, When the
//! admin calls `admin.signup_pause` with a message, Then the pause file holds
//! it as line 1, the result is `{paused, changed, message, retry_after_secs:
//! 3600}`, admin healthz reports `signups_enabled: false` + the message, and
//! `signup` returns `signup_paused` with it; a hand-written file is replaced
//! the same way.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

async fn healthz(base_url: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

async fn assert_paused_with(server: &TestServer, admin: &McpClient, message: &str) {
    let result = admin
        .tools_call("admin.signup_pause", json!({"message": message}))
        .await
        .expect("admin.signup_pause");
    let out = extract_structured(&result);
    assert_eq!(
        out,
        json!({"paused": true, "changed": true, "message": message, "retry_after_secs": 3600}),
        "{out:?}"
    );

    let file = std::fs::read_to_string(server.state.signup_pause.path()).expect("pause file");
    assert_eq!(file.lines().next(), Some(message), "{file:?}");

    let health = healthz(&server.base_url).await;
    assert_eq!(health["signups_enabled"], json!(false), "{health:?}");
    assert_eq!(health["signup_pause_message"], json!(message), "{health:?}");

    let err = McpClient::new(&server.base_url)
        .tools_call("signup", json!({"name": "While Paused"}))
        .await
        .expect_err("signup while paused must fail");
    assert_eq!(err.error_code.as_deref(), Some("signup_paused"), "{err:?}");
    assert_eq!(err.message, message, "{err:?}");
}

#[tokio::test]
async fn aspt_ac02_pause_with_no_file_writes_it_and_refuses_signup() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    assert!(!server.state.signup_pause.path().exists());
    assert_paused_with(&server, &admin, "mtproof probe: signups paused").await;
}

#[tokio::test]
async fn aspt_ac02_pause_replaces_a_hand_written_file() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    std::fs::write(server.state.signup_pause.path(), "hand written\nuntil=9999999999\n").expect("hand file");
    assert_paused_with(&server, &admin, "tool message").await;
    let file = std::fs::read_to_string(server.state.signup_pause.path()).expect("pause file");
    assert_eq!(file, "tool message\n", "the hand-written until line must be replaced too");
}
