//! PRD-mcphost-invite-links
//! AC2 (P0) — Given that URL, When a fresh session POSTs `tools/call
//! host.tool_call {name: "<A ns>.tt"}` with no header, Then a new tenant B
//! is created with `source: "invite:<code>"`, the call succeeds, the
//! response carries `onboarding.url`, `onboarding.invited_by: <A ns>`,
//! and `onboarding.shared_tools: ["tt"]`, and `uses` is 1.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn first_call_through_invite_creates_tenant_and_onboards() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC2 Inviter").await;
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
    let url = created["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let invitee = McpClient::new(&server.base_url)
        .with_path(&path)
        .with_session_continuity();
    let call = invitee
        .tools_call(
            "host.tool_call",
            json!({"name": format!("{ns_a}.tt"), "args": {}}),
        )
        .await
        .expect("host.tool_call through the invite with no credential");
    let result = extract_structured(&call);

    let onboarding = &result["onboarding"];
    assert!(
        onboarding["url"].as_str().is_some_and(|u| u.contains("/u/")),
        "{result}"
    );
    assert_eq!(onboarding["invited_by"], json!(ns_a), "{result}");
    assert_eq!(onboarding["shared_tools"], json!(["tt"]), "{result}");

    // `uses` is now 1 on A's side.
    let listed = client_a
        .tools_call("host.invite.list", json!({}))
        .await
        .expect("host.invite.list");
    let listed = extract_structured(&listed);
    let created_invite = listed["invites"]
        .as_array()
        .expect("invites array")
        .iter()
        .find(|i| i["kind"] == json!("created"))
        .expect("created invite");
    assert_eq!(created_invite["uses"], json!(1), "{listed}");

    // The invitee tenant exists and was stamped with the invite source.
    let invitee_ns = onboarding["tenant"].as_str().expect("onboarding.tenant").to_string();
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(invitee_ns)
        .await
        .expect("db query")
        .expect("invitee tenant exists");
    assert!(
        tenant.signup_source.as_deref().is_some_and(|s| s.starts_with("invite:")),
        "{:?}",
        tenant.signup_source
    );
}
