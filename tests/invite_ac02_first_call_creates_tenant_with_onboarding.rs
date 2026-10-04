//! PRD-mcphost-invite-links
//! AC2 — Given an invite url, When a fresh session POSTs `tools/call
//! host.tool.call {name: "<A ns>.mytool"}` with no header, Then a new tenant B
//! is created with `source: "invite:<code>"`, the call succeeds, the
//! response carries `onboarding.url`, `onboarding.invited_by: <A ns>`, and
//! `onboarding.shared_tools: ["mytool"]`, and `uses` is 1.
//!
//! Also covers requirement 6 (no AC of its own): a second call on the
//! same session continues as tenant B, not a second tenant.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn first_call_on_invite_path_creates_tenant_with_onboarding() {
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

    let created = a_client
        .tools_call("host.invite.create", json!({"share": ["mytool"]}))
        .await
        .expect("invite create should succeed");
    let url = extract_structured(&created)["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let b_session = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = b_session
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect("the first call on the invite path must succeed");
    let first = extract_structured(&first_call);

    let onboarding = &first["onboarding"];
    let b_url = onboarding["url"].as_str().expect("onboarding.url present");
    let b_path = b_url.trim_start_matches(&server.base_url);
    assert!(
        b_path.starts_with("/u/") && b_path.ends_with("/mcp"),
        "onboarding.url must be a /u/<secret>/mcp URL, got {b_url}"
    );
    assert_eq!(onboarding["invited_by"], json!(a_ns));
    assert_eq!(onboarding["shared_tools"], json!(["mytool"]));

    // requirement 6: a second call on the same session continues as the
    // same tenant B -- no second onboarding, no second tenant.
    let second_call = b_session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("second call on the same session must succeed as tenant B");
    let second = extract_structured(&second_call);
    assert!(
        second.get("onboarding").is_none(),
        "a later call on the same session must not carry onboarding again: {second}"
    );

    let listed = a_client
        .tools_call("host.invite.list", json!({}))
        .await
        .expect("invite list should succeed");
    let listed = extract_structured(&listed);
    let entry = listed["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == json!("standard"))
        .expect("the standard invite must be listed");
    assert_eq!(entry["uses"], json!(1));
}
