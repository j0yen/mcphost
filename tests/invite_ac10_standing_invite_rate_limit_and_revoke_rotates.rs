//! PRD-mcphost-invite-links
//! AC10 — Given tenant A's standing invite, When 21 fresh sessions join
//! through it within one hour, Then 20 tenants are created and the 21st
//! receives `invite_rate_limited`; When A calls `host.invite.revoke` on
//! it, Then the response carries a new standing `invite_url` and the old
//! URL returns `invite_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn standing_invite_caps_at_twenty_per_hour_and_revoke_rotates_it() {
    let server = TestServer::start().await;
    let (_a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

    let whoami = extract_structured(
        &a_client.tools_call("host.whoami", json!({})).await.expect("whoami should succeed"),
    );
    let old_url = whoami["invite_url"].as_str().expect("invite_url present").to_string();
    let old_path = old_url.trim_start_matches(&server.base_url).to_string();
    let old_code = old_path.strip_prefix("/i/").unwrap().strip_suffix("/mcp").unwrap().to_string();

    for n in 0..20 {
        let client = McpClient::new(&server.base_url).with_path(&old_path);
        client
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("join {n} of 20 through the standing invite must succeed: {e:?}"));
    }
    let client21 = McpClient::new(&server.base_url).with_path(&old_path);
    let err = client21
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("the 21st join within the hour must be refused");
    assert_eq!(err.error_code, Some("invite_rate_limited".to_string()));

    let revoked = extract_structured(
        &a_client
            .tools_call("host.invite.revoke", json!({"code": old_code}))
            .await
            .expect("revoking the standing invite should succeed"),
    );
    let new_url = revoked["invite_url"].as_str().expect("revoke returns a new standing invite_url");
    assert_ne!(new_url, old_url, "the rotated url must differ from the old one");

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}{old_path}", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST to the old, now-rotated-away standing invite url");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    // The new standing invite_url works.
    let new_path = new_url.trim_start_matches(&server.base_url).to_string();
    let new_client = McpClient::new(&server.base_url).with_path(&new_path);
    new_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("the newly rotated standing invite must work");
}
