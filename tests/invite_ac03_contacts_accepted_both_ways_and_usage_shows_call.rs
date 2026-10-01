//! PRD-mcphost-invite-links
//! AC3 (P0) — Given tenant B, When `host.agent.contacts` is called from
//! A and from B, Then each lists the other as accepted with
//! `via: "invite:<code>"`, and `host.usage by=caller tool=t` on A shows
//! B's call.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn contacts_accepted_both_ways_and_usage_attributes_to_invitee() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC3 Inviter").await;
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
    let url = extract_structured(&created)["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let invitee = McpClient::new(&server.base_url)
        .with_path(&path)
        .with_session_continuity();
    let call = invitee
        .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
        .await
        .expect("host.tool_call through the invite");
    let ns_b = extract_structured(&call)["onboarding"]["tenant"]
        .as_str()
        .expect("onboarding.tenant")
        .to_string();

    let contacts_a = client_a
        .tools_call("host.agent.contacts", json!({}))
        .await
        .expect("host.agent.contacts from A");
    let contacts_a = extract_structured(&contacts_a);
    let a_sees_b = contacts_a["contacts"]
        .as_array()
        .expect("contacts array")
        .iter()
        .find(|c| c["address"] == json!(ns_b))
        .expect("A sees B as a contact");
    assert!(
        a_sees_b["via"].as_str().is_some_and(|v| v.starts_with("invite:")),
        "{a_sees_b}"
    );

    // B's own call runs on the session that was just bound to it.
    let whoami_b = invitee
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the bound invitee session");
    assert_eq!(extract_structured(&whoami_b)["tenant"], json!(ns_b));

    let contacts_b = invitee
        .tools_call("host.agent.contacts", json!({}))
        .await
        .expect("host.agent.contacts from B");
    let contacts_b = extract_structured(&contacts_b);
    let b_sees_a = contacts_b["contacts"]
        .as_array()
        .expect("contacts array")
        .iter()
        .find(|c| c["address"] == json!(ns_a))
        .expect("B sees A as a contact");
    assert!(
        b_sees_a["via"].as_str().is_some_and(|v| v.starts_with("invite:")),
        "{b_sees_a}"
    );

    let usage = client_a
        .tools_call("host.usage", json!({"by": "caller", "tool": "tt"}))
        .await
        .expect("host.usage by=caller tool=t on A");
    let usage = extract_structured(&usage);
    let rows = usage["rows"].as_array().expect("rows array");
    let b_row = rows
        .iter()
        .find(|r| r["key"] == json!(ns_b))
        .expect("B's call shows up in A's caller breakdown");
    assert!(b_row["calls"].as_i64().unwrap_or(0) >= 1, "{usage}");
}
