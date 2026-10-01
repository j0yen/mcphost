//! PRD-mcphost-invite-links
//! AC1 (P0) — Given inviter tenant A with a published tool `t`, When A
//! calls `host.invite.create share=["tt"]`, Then the result has a `url`
//! of the form `/i/<code>/mcp`, `max_uses: 10`, `expires_at` 30 days out,
//! and `host.invite.list` shows it with `uses: 0`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn create_returns_url_defaults_and_list_shows_zero_uses() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Inviter").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "tt", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish t");

    let before = mcphost::state::now_unix();
    let created = client
        .tools_call("host.invite.create", json!({"share": ["tt"]}))
        .await
        .expect("host.invite.create");
    let created = extract_structured(&created);

    let url = created["url"].as_str().expect("url field").to_string();
    assert!(
        url.starts_with(&format!("{}/i/", server.base_url)) && url.ends_with("/mcp"),
        "url must be /i/<code>/mcp: {url}"
    );
    assert_eq!(created["max_uses"], json!(10), "{created}");
    assert_eq!(created["uses"], json!(0), "{created}");

    let expires_at = created["expires_at"].as_str().expect("expires_at").to_string();
    let expected_date = mcphost::state::rfc3339_from_unix(before + 30 * 86_400)[..10].to_string();
    assert_eq!(
        &expires_at[..10],
        expected_date,
        "expires_at must be ~30 days out: {created}"
    );

    let listed = client
        .tools_call("host.invite.list", json!({}))
        .await
        .expect("host.invite.list");
    let listed = extract_structured(&listed);
    let invites = listed["invites"].as_array().expect("invites array");
    let created_invite = invites
        .iter()
        .find(|i| i["kind"] == json!("created"))
        .expect("a created invite in the list");
    assert_eq!(created_invite["uses"], json!(0), "{listed}");
    assert_eq!(created_invite["max_uses"], json!(10), "{listed}");
    assert_eq!(created_invite["share"], json!(["tt"]), "{listed}");
}
