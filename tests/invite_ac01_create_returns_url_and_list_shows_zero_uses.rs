//! PRD-mcphost-invite-links
//! AC1 — Given inviter tenant A with a published tool `t`, When A calls
//! `host.invite.create share=["mytool"]`, Then the result has a `url` of the
//! form `/i/<code>/mcp`, `max_uses: 10`, `expires_at` 30 days out, and
//! `host.invite.list` shows it with `uses: 0`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn create_returns_url_shape_and_list_shows_zero_uses() {
    let server = TestServer::start().await;
    let (_tenant_ns, key) = signup(&server.base_url, "Inviter A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
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

    let before = mcphost::state::now_unix();
    let created = client
        .tools_call("host.invite.create", json!({"share": ["mytool"]}))
        .await
        .expect("invite create should succeed");
    let result = extract_structured(&created);

    let url = result["url"].as_str().expect("url present");
    let path = url.trim_start_matches(&server.base_url);
    assert!(
        path.starts_with("/i/") && path.ends_with("/mcp"),
        "url must be /i/<code>/mcp, got {url}"
    );
    let code = path.strip_prefix("/i/").unwrap().strip_suffix("/mcp").unwrap();
    assert!(code.len() >= 20, "code must be 20+ characters, got {code:?}");
    assert_eq!(result["max_uses"], json!(10));
    assert_eq!(result["share"], json!(["mytool"]));

    let expires_at = result["expires_at"].as_i64().expect("expires_at present");
    let thirty_days = 30 * 24 * 3_600;
    assert!(
        (expires_at - before - thirty_days).abs() < 60,
        "expires_at must be ~30 days out: expires_at={expires_at} before={before}"
    );

    let listed = client
        .tools_call("host.invite.list", json!({}))
        .await
        .expect("invite list should succeed");
    let listed = extract_structured(&listed);
    let invites = listed["invites"].as_array().expect("invites array");
    let entry = invites
        .iter()
        .find(|i| i["kind"] == json!("standard"))
        .expect("the newly created standard invite must be listed");
    assert_eq!(entry["uses"], json!(0));
    assert_eq!(entry["max_uses"], json!(10));
    assert_eq!(entry["revoked"], json!(false));
    assert_eq!(entry["invitees"], json!([]));
}
