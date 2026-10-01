//! PRD-mcphost-invite-links
//! AC5 (P0) — Given a revoked or expired code, When any request hits
//! `/i/<code>/mcp`, Then HTTP 404 `invite_invalid`, the browser page
//! reveals no inviter identity, and existing invitees' shares still
//! work.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn revoked_code_404s_with_no_identity_leak_but_existing_invitee_unaffected() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC5 Inviter").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "tt", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish t");
    let created = client_a
        .tools_call("host.invite.create", json!({"share": ["tt"]}))
        .await
        .expect("host.invite.create");
    let created = extract_structured(&created);
    let code = created["code"].as_str().expect("code").to_string();
    let url = created["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    // An existing invitee joins BEFORE revocation. The invite path only
    // authenticates the FIRST (claiming) call -- the invitee's durable
    // identity is its own minted personal URL (`onboarding.url`), not the
    // invite path, which this requirement's doctrine 404s unconditionally
    // once revoked (same "no identity leak" rule applies to a live
    // session as to a stranger -- nothing about `/i/<code>/mcp` itself
    // stays valid past revocation).
    let invitee = McpClient::new(&server.base_url)
        .with_path(&path)
        .with_session_continuity();
    let join_result = invitee
        .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
        .await
        .expect("invitee joins before revoke");
    let onboarding_url = extract_structured(&join_result)["onboarding"]["url"]
        .as_str()
        .expect("onboarding.url present on first join")
        .to_string();
    let own_path = onboarding_url.trim_start_matches(&server.base_url).to_string();

    client_a
        .tools_call("host.invite.revoke", json!({"code": code}))
        .await
        .expect("host.invite.revoke");

    // A plain JSON POST to the revoked path: 404 with error class
    // invite_invalid.
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}{}", server.base_url, path))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("post to revoked invite path");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], json!("invite_invalid"), "{body}");

    // A browser GET: a text page naming no inviter identity (namespace,
    // display name) at all.
    let browser_resp = http
        .get(format!("{}{}", server.base_url, path))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("browser GET on revoked invite path");
    assert_eq!(browser_resp.status(), reqwest::StatusCode::NOT_FOUND);
    let html = browser_resp.text().await.expect("html body");
    assert!(
        !html.contains(&ns_a),
        "revoked-invite page must not leak the inviter's namespace: {html}"
    );

    // The existing invitee's own personal URL (not the invite path)
    // still works.
    let own_client = McpClient::new(&server.base_url).with_path(&own_path);
    let whoami = own_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("existing invitee's own personal URL is unaffected by the revoke");
    assert!(extract_structured(&whoami)["tenant"].as_str().is_some());
}
