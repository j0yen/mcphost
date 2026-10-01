//! PRD-mcphost-invite-links
//! AC10 (P0) — Given tenant A's standing invite, When 21 fresh sessions
//! join through it within one hour, Then 20 tenants are created and the
//! 21st receives `invite_rate_limited`; When A calls
//! `host.invite.revoke` on it, Then the response carries a new standing
//! `invite_url` and the old URL returns `invite_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn standing_invite_rate_limited_then_revoke_rotates() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "AC10 Inviter").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    let whoami_a = client_a.tools_call("host.whoami", json!({})).await.expect("host.whoami");
    let standing_url = extract_structured(&whoami_a)["invite_url"]
        .as_str()
        .expect("standing invite_url")
        .to_string();
    let path = standing_url.trim_start_matches(&server.base_url).to_string();
    let code = path.trim_start_matches("/i/").trim_end_matches("/mcp").to_string();

    for i in 0..20 {
        let session = McpClient::new(&server.base_url)
            .with_path(&path)
            .with_session_continuity();
        session
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("standing-invite join #{i} must succeed: {} {}", e.code, e.message));
    }
    let session21 = McpClient::new(&server.base_url)
        .with_path(&path)
        .with_session_continuity();
    let err = session21
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("the 21st standing-invite join within the hour must be refused");
    assert_eq!(err.error_code.as_deref(), Some("invite_rate_limited"), "{err:?}");

    let rotated = client_a
        .tools_call("host.invite.revoke", json!({"code": code}))
        .await
        .expect("host.invite.revoke on the standing invite");
    let rotated = extract_structured(&rotated);
    assert_eq!(rotated["kind"], json!("standing"), "{rotated}");
    let new_url = rotated["url"].as_str().expect("new standing url").to_string();
    assert_ne!(new_url, standing_url, "revoke must rotate to a different URL");

    // The old URL is now invalid.
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}{}", server.base_url, path))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("post to the old standing-invite path");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}
