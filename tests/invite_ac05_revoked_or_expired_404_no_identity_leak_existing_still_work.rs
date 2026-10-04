//! PRD-mcphost-invite-links
//! AC5 — Given a revoked or expired code, When any request hits
//! `/i/<code>/mcp`, Then HTTP 404 `invite_invalid`, the browser page
//! reveals no inviter identity, and existing invitees' shares still work.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn assert_404_no_identity_leak(http: &reqwest::Client, base_url: &str, path: &str, a_ns: &str) {
    let resp = http
        .post(format!("{base_url}{path}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST to an invalid invite code");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    let body = resp.text().await.expect("read body");
    assert!(
        !body.contains(a_ns),
        "a 404 for an invalid invite must never name the inviter: {body}"
    );

    let resp = http
        .get(format!("{base_url}{path}"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("GET an invalid invite code");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    let body = resp.text().await.expect("read body");
    assert!(
        !body.contains(a_ns),
        "the browser page for an invalid invite must never name the inviter: {body}"
    );
}

#[tokio::test]
async fn revoked_code_404s_with_no_identity_leak_and_existing_shares_still_work() {
    let server = TestServer::start().await;
    let (a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);
    let http = reqwest::Client::new();

    a_client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "mytool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("publish should succeed");
    let created = extract_structured(
        &a_client
            .tools_call("host.invite.create", json!({"share": ["mytool"]}))
            .await
            .expect("invite create should succeed"),
    );
    let url = created["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let code = path.strip_prefix("/i/").unwrap().strip_suffix("/mcp").unwrap().to_string();

    // B joins before revocation -- its share must survive the revoke below.
    // The first call's own onboarding.url is B's OWN personal URL (PRD-
    // mcphost-url-bound-tenants) -- the channel an invitee is meant to keep
    // using afterward, independent of whatever happens to the invite link
    // it arrived on.
    let b_session = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first = extract_structured(
        &b_session
            .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
            .await
            .expect("B's first call must succeed before revocation"),
    );
    let b_url = first["onboarding"]["url"].as_str().expect("onboarding.url").to_string();
    let b_path = b_url.trim_start_matches(&server.base_url).to_string();

    a_client
        .tools_call("host.invite.revoke", json!({"code": code}))
        .await
        .expect("revoke should succeed");

    assert_404_no_identity_leak(&http, &server.base_url, &path, &a_ns).await;

    // Existing invitee's share still works after the code is revoked --
    // via B's own personal URL, since the invite link itself is now dead.
    let b_client = McpClient::new(&server.base_url).with_path(&b_path);
    b_client
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect("B's existing share must keep working after the invite is revoked");
}

#[tokio::test]
async fn expired_code_404s_with_no_identity_leak() {
    let server = TestServer::start().await;
    let (a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);
    let http = reqwest::Client::new();

    let created = extract_structured(
        &a_client
            .tools_call("host.invite.create", json!({}))
            .await
            .expect("invite create should succeed"),
    );
    let url = created["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let code = path.strip_prefix("/i/").unwrap().strip_suffix("/mcp").unwrap().to_string();

    server
        .state
        .db
        .test_backdate_invite_expiry(mcphost::auth::hash_key(&code), mcphost::state::now_unix() - 1)
        .await
        .expect("backdate expiry");

    assert_404_no_identity_leak(&http, &server.base_url, &path, &a_ns).await;
}
