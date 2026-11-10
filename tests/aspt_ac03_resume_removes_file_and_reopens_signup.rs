//! AC3 (PRD-mcphost-admin-signup-pause-tool) — Given a pause file written by
//! the tool or by hand, When the admin calls `admin.signup_pause` with
//! `resume: true`, Then the file is gone, the result is `{paused: false,
//! changed: true}`, admin healthz reports `signups_enabled: true` with no
//! `signup_pause_message`, and a `signup` succeeds (the tenant it creates is
//! offboarded).

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

async fn resume_and_check(server: &TestServer, admin: &McpClient) {
    let result = admin
        .tools_call("admin.signup_pause", json!({"resume": true}))
        .await
        .expect("admin.signup_pause resume");
    assert_eq!(extract_structured(&result), json!({"paused": false, "changed": true}));
    assert!(!server.state.signup_pause.path().exists(), "pause file must be gone");

    let health = healthz(&server.base_url).await;
    assert_eq!(health["signups_enabled"], json!(true), "{health:?}");
    assert!(health.get("signup_pause_message").is_none(), "{health:?}");

    let signed_up = McpClient::new(&server.base_url)
        .tools_call("signup", json!({"name": "After Tool Resume"}))
        .await
        .expect("signup after resume must succeed");
    let tenant = extract_structured(&signed_up)["tenant"].as_str().expect("tenant").to_string();
    admin
        .tools_call("admin.tenant_delete", json!({"tenant": tenant}))
        .await
        .expect("offboard the tenant the test created");
}

#[tokio::test]
async fn aspt_ac03_resume_removes_a_tool_written_file() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.signup_pause", json!({"message": "paused by tool"}))
        .await
        .expect("pause");
    assert!(server.state.signup_pause.path().exists());
    resume_and_check(&server, &admin).await;
}

#[tokio::test]
async fn aspt_ac03_resume_removes_a_hand_written_file() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    std::fs::write(server.state.signup_pause.path(), "by hand\n").expect("hand file");
    resume_and_check(&server, &admin).await;
}
