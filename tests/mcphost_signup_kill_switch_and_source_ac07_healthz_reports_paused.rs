//! AC7 (PRD-mcphost-signup-kill-switch-and-source) — Given the pause file
//! exists, When admin healthz is read, Then `signups_enabled` is false and
//! `signup_pause_message` matches the file.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
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

#[tokio::test]
async fn healthz_reports_disabled_and_pause_message_while_paused() {
    let server = TestServer::start().await;

    let before = healthz(&server.base_url).await;
    assert_eq!(before["signups_enabled"], json!(true), "{before:?}");
    assert!(before.get("signup_pause_message").is_none(), "{before:?}");

    std::fs::write(server.state.signup_pause.path(), "ac7 pause message\nuntil=9999999999\n")
        .expect("write pause file");

    let after = healthz(&server.base_url).await;
    assert_eq!(after["signups_enabled"], json!(false), "{after:?}");
    assert_eq!(after["signup_pause_message"], json!("ac7 pause message"), "{after:?}");
}
