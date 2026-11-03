//! AC6 (PRD-mcphost-admin-signup-pause-tool) — Given no pause file, When the
//! admin calls `resume: true` twice, Then both return `{paused: false,
//! changed: false}`; and Given `MCPHOST_SIGNUP_PAUSE_FILE` points into a
//! directory that does not exist, When the admin calls `message: "x"`, Then
//! the error code is `storage`, nothing panics, and healthz still answers
//! `signups_enabled: true`.

use crate::common;
use common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured};
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

#[tokio::test]
async fn aspt_ac06_resume_with_no_file_twice_is_a_quiet_no_op() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for _ in 0..2 {
        let result = admin
            .tools_call("admin.signup_pause", json!({"resume": true}))
            .await
            .expect("resume with no file is not an error");
        assert_eq!(extract_structured(&result), json!({"paused": false, "changed": false}));
    }
}

#[tokio::test]
async fn aspt_ac06_missing_parent_directory_is_a_storage_error() {
    let scratch = std::env::temp_dir().join(format!("aspt-ac06-{}", std::process::id()));
    let missing = scratch.join("no-such-dir").join("signup.paused");
    let server = {
        // Held only across construction: `SignupPause::from_env` reads the var
        // once, so the state keeps the path after the guard is released.
        let _guard = EnvGuard::set("MCPHOST_SIGNUP_PAUSE_FILE", missing.to_str().expect("utf-8 path")).await;
        TestServer::start().await
    };
    assert_eq!(server.state.signup_pause.path(), missing.as_path());
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let err = admin
        .tools_call("admin.signup_pause", json!({"message": "x"}))
        .await
        .expect_err("a write into a missing directory must fail");
    assert_eq!(err.error_code.as_deref(), Some("storage"), "{err:?}");
    assert!(!missing.exists());

    let health = healthz(&server.base_url).await;
    assert_eq!(health["signups_enabled"], json!(true), "{health:?}");

    // Resume on the same unreachable path is still the idempotent no-op.
    let result = admin
        .tools_call("admin.signup_pause", json!({"resume": true}))
        .await
        .expect("resume of an absent file");
    assert_eq!(extract_structured(&result), json!({"paused": false, "changed": false}));
}
