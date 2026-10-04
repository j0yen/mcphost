//! PRD-mcphost-invite-links
//! AC9 — Given a freshly created tenant (by `signup`, implicit signup,
//! URL page, or invite), When `host.whoami` runs, Then `invite_url` is
//! present, of the form `/i/<code>/mcp`, `host.invite.list` shows it with
//! `kind: "standing"`, no `expires_at`, no `max_uses`, and it is not
//! counted in `invites_max`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn standing_invite_present_at_whoami_and_not_counted_in_invites_max() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Fresh Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let whoami = extract_structured(
        &client.tools_call("host.whoami", json!({})).await.expect("whoami should succeed"),
    );
    let invite_url = whoami["invite_url"].as_str().expect("invite_url present");
    let path = invite_url.trim_start_matches(&server.base_url);
    assert!(
        path.starts_with("/i/") && path.ends_with("/mcp"),
        "invite_url must be /i/<code>/mcp, got {invite_url}"
    );

    let listed = extract_structured(
        &client.tools_call("host.invite.list", json!({})).await.expect("invite list should succeed"),
    );
    let standing = listed["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == json!("standing"))
        .expect("the standing invite must be listed");
    assert!(standing.get("expires_at").is_none(), "standing invite must carry no expires_at: {standing}");
    assert!(standing.get("max_uses").is_none(), "standing invite must carry no max_uses: {standing}");

    // Not counted in invites_max: the free plan's 3-live-standard-invite
    // cap is unaffected by the standing invite that already exists.
    for n in 0..3 {
        client
            .tools_call("host.invite.create", json!({}))
            .await
            .unwrap_or_else(|e| panic!("standard invite {n} of 3 within the free cap must succeed: {e:?}"));
    }
    let err = client
        .tools_call("host.invite.create", json!({}))
        .await
        .expect_err("a 4th standard invite must still be refused by the free cap");
    assert_eq!(err.error_code, Some("invites_quota_exceeded".to_string()));
    assert_eq!(
        err.data["invites_max"],
        json!(3),
        "the standing invite must not count toward invites_max"
    );
}
