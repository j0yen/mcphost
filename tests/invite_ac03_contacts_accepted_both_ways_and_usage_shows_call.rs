//! PRD-mcphost-invite-links
//! AC3 — Given tenant B (created via an invite from A), When
//! `host.agent.contacts` is called from A and from B, Then each lists the
//! other as accepted with `via: "invite:<code>"`, and `host.usage
//! by=caller tool=t` on A shows B's call.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn contacts_accepted_both_ways_and_usage_shows_callers_call() {
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
    let created = extract_structured(&created);
    let url = created["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let code = path.strip_prefix("/i/").unwrap().strip_suffix("/mcp").unwrap().to_string();

    let b_session = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = b_session
        .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
        .await
        .expect("the first call on the invite path must succeed");
    let b_ns = extract_structured(&first_call)["onboarding"]["invited_by"]
        .as_str()
        .map(str::to_string);
    assert_eq!(b_ns, Some(a_ns.clone()), "sanity: onboarding.invited_by is A");

    let a_contacts = extract_structured(
        &a_client
            .tools_call("host.agent.contacts", json!({}))
            .await
            .expect("A's contacts should succeed"),
    );
    let via_expected = format!("invite:{code}");
    let a_entry = a_contacts["contacts"]
        .as_array()
        .expect("contacts array")
        .iter()
        .find(|c| c["via"] == json!(via_expected))
        .expect("A must list B as an accepted contact via the invite");
    let b_namespace = a_entry["address"].as_str().unwrap().to_string();

    let b_contacts = extract_structured(
        &b_session
            .tools_call("host.agent.contacts", json!({}))
            .await
            .expect("B's contacts should succeed"),
    );
    let b_entry = b_contacts["contacts"]
        .as_array()
        .expect("contacts array")
        .iter()
        .find(|c| c["address"] == json!(a_ns))
        .expect("B must list A as an accepted contact");
    assert_eq!(b_entry["via"], json!(via_expected));

    let usage = extract_structured(
        &a_client
            .tools_call("host.usage", json!({"by": "caller", "tool": "mytool"}))
            .await
            .expect("usage by=caller should succeed"),
    );
    let rows = usage["rows"].as_array().expect("rows array");
    let row = rows
        .iter()
        .find(|r| r["key"] == json!(b_namespace))
        .expect("B's call must show up in A's by=caller usage");
    assert!(row["calls"].as_i64().unwrap() >= 1);
}
