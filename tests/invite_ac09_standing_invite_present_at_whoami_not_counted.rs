//! PRD-mcphost-invite-links
//! AC9 (P0) — Given a freshly created tenant (by `signup`, implicit
//! signup, URL page, or invite), When `host.whoami` runs, Then
//! `invite_url` is present, of the form `/i/<code>/mcp`,
//! `host.invite.list` shows it with `kind: "standing"`, no `expires_at`,
//! no `max_uses`, and it is not counted in `invites_max`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn standing_invite_present_at_whoami_and_not_counted_toward_the_cap() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let whoami = client.tools_call("host.whoami", json!({})).await.expect("host.whoami");
    let whoami = extract_structured(&whoami);
    let invite_url = whoami["invite_url"].as_str().expect("invite_url present on whoami");
    assert!(
        invite_url.starts_with(&format!("{}/i/", server.base_url)) && invite_url.ends_with("/mcp"),
        "{invite_url}"
    );

    let listed = client.tools_call("host.invite.list", json!({})).await.expect("host.invite.list");
    let listed = extract_structured(&listed);
    let standing = listed["invites"]
        .as_array()
        .expect("invites array")
        .iter()
        .find(|i| i["kind"] == json!("standing"))
        .expect("a standing invite in the list");
    assert_eq!(standing["expires_at"], json!(None::<String>), "{standing}");
    assert_eq!(standing["max_uses"], json!(None::<i64>), "{standing}");
    assert_eq!(standing["revoked"], json!(false), "{standing}");

    // The standing invite never counts toward `invites_max`: three
    // `"created"` invites plus the standing one must still leave the
    // free-plan tenant able to make exactly 3 lives, not 2.
    for _ in 0..3 {
        client
            .tools_call("host.invite.create", json!({}))
            .await
            .expect("created invites still get the full invites_max budget");
    }
    let err = client
        .tools_call("host.invite.create", json!({}))
        .await
        .expect_err("a 4th created invite is still refused");
    assert_eq!(err.error_code.as_deref(), Some("invites_max"));
}
