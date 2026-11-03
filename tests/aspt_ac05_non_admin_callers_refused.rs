//! AC5 (PRD-mcphost-admin-signup-pause-tool) — Given a tenant key and an
//! anonymous client, When each calls `admin.signup_pause` with `message: "x"`,
//! Then each is refused with the same error code every other `admin.*` tool
//! returns to that caller (`forbidden` for a tenant key), no pause file is
//! created, and healthz still reports `signups_enabled: true`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
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

async fn assert_refused_like_other_admin_tools(server: &TestServer, caller: &McpClient) -> Option<String> {
    let reference = caller
        .tools_call("admin.tenants", json!({}))
        .await
        .expect_err("a non-admin caller must be refused by admin.tenants");
    let err = caller
        .tools_call("admin.signup_pause", json!({"message": "x"}))
        .await
        .expect_err("a non-admin caller must be refused by admin.signup_pause");
    assert_eq!(err.error_code, reference.error_code, "{err:?} vs {reference:?}");
    assert!(!server.state.signup_pause.path().exists(), "no pause file may be created");
    let health = healthz(&server.base_url).await;
    assert_eq!(health["signups_enabled"], json!(true), "{health:?}");
    err.error_code
}

#[tokio::test]
async fn aspt_ac05_tenant_key_is_forbidden() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Not An Admin").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    let code = assert_refused_like_other_admin_tools(&server, &tenant).await;
    assert_eq!(code.as_deref(), Some("forbidden"));
}

#[tokio::test]
async fn aspt_ac05_anonymous_client_gets_the_same_code_as_other_admin_tools() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);
    let code = assert_refused_like_other_admin_tools(&server, &anon).await;
    assert!(code.is_some(), "the refusal must carry an error code");
}
