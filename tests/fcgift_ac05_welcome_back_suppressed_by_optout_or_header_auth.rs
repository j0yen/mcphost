//! PRD-mcphost-first-call-gift
//! AC5 — Given a tenant that called
//! `host.agent.profile_set(welcome_back = false)`, or a session
//! authenticated by `Authorization` header, When a new session's first
//! call succeeds, Then no `welcome_back` field is present.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn opted_out_tenant_sees_no_welcome_back_on_a_new_session() {
    let server = TestServer::start().await;

    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    session1
        .tools_call("signup", json!({"name": "AC5a Tenant", "remember": "cleaning Joe's CSVs"}))
        .await
        .expect("signup with remember");
    session1
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the earlier session");
    session1
        .tools_call("host.agent.profile_set", json!({"welcome_back": false}))
        .await
        .expect("opt out of welcome_back");
    let rotated = extract_structured(
        &session1
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a personal URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = extract_structured(
        &session2
            .tools_call("host.whoami", json!({}))
            .await
            .expect("first call of the new URL-bound session"),
    );
    assert!(
        first_call.get("welcome_back").is_none(),
        "an opted-out tenant must see no welcome_back: {first_call}"
    );
}

#[tokio::test]
async fn header_authenticated_session_sees_no_welcome_back() {
    let server = TestServer::start().await;

    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    let signup = extract_structured(
        &session1
            .tools_call("signup", json!({"name": "AC5b Tenant", "remember": "cleaning Joe's CSVs"}))
            .await
            .expect("signup with remember"),
    );
    let key = signup["key"].as_str().expect("key").to_string();
    session1
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the earlier session");

    // A brand new, differently-configured client, authenticated by
    // `Authorization: Bearer` header rather than session binding or a
    // `/u/{secret}/mcp` path -- never eligible for welcome_back regardless
    // of session continuity.
    let header_client = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let first_call = extract_structured(
        &header_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("first call authenticated by Authorization header"),
    );
    assert!(
        first_call.get("welcome_back").is_none(),
        "a header-authenticated session must see no welcome_back: {first_call}"
    );
}
