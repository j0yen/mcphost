//! PRD-mcphost-invite-links
//! AC13 — Given invitee B calling `<A ns>.t` for the first time in a
//! session, When the result returns, Then `_meta.invite_url` equals B's
//! standing invite URL, and the second call in the same session carries
//! no `_meta.invite_url`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn meta_invite_url_hint_shows_once_per_session() {
    let server = TestServer::start().await;
    let (a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

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
    let path = created["url"]
        .as_str()
        .unwrap()
        .trim_start_matches(&server.base_url)
        .to_string();

    let b_session = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_raw = b_session
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect("B's first call must succeed");
    let first_onboarding_url = extract_structured(&first_raw)["onboarding"]["url"]
        .as_str()
        .expect("onboarding.url present on the first call")
        .to_string();
    let b_hint_url = first_raw["_meta"]["invite_url"]
        .as_str()
        .expect("_meta.invite_url present on the first shared-tool call")
        .to_string();
    assert!(
        b_hint_url.starts_with(&server.base_url) && b_hint_url.contains("/i/"),
        "invite_url hint must be B's own standing invite url: {b_hint_url}"
    );
    assert_ne!(
        b_hint_url, first_onboarding_url,
        "the hint is B's own STANDING invite url, distinct from onboarding's /u/ URL"
    );

    let second_raw = b_session
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect("B's second call must succeed");
    assert!(
        second_raw.get("_meta").and_then(|m| m.get("invite_url")).is_none(),
        "the second call in the same session must carry no _meta.invite_url: {second_raw}"
    );
}
