//! PRD-mcphost-org-shared-tools-namespace-v2
//! AC3 — Given M's keyed `tools/list`, When O has 2 org tools `t`, `u` and M
//! has a private `t`, Then the list holds `t`, `org.t`, `org.u` and nothing
//! from a tenant outside the org, and a tenant X outside the org lists neither
//! `org.t` nor `org.u`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::{Value, json};

fn echo() -> Value {
    json!({"schema": {"type": "object"}})
}

/// Every published-tool name (`<namespace>.<tool>` or `org.<tool>`) a keyed
/// `tools/list` carries, host verbs left out.
async fn listed(client: &McpClient) -> Vec<String> {
    let list = client.tools_list().await.expect("tools/list");
    let mut names: Vec<String> = list["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .filter(|n| n.contains('.') && !n.starts_with("host.") && !n.starts_with("billing.") && !n.starts_with("admin."))
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[tokio::test]
async fn orgs_ac03() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"));
    let code = created["invite_url"]
        .as_str()
        .expect("invite_url")
        .trim_end_matches("/mcp")
        .rsplit('/')
        .next()
        .expect("code")
        .to_string();
    let (member_ns, member_key) = signup(&server.base_url, "Member").await;
    let member = McpClient::with_bearer(&server.base_url, &member_key);
    member.tools_call("host.org.join", json!({"code": code})).await.expect("join");

    for name in ["tt", "uu"] {
        publish(&owner, name, "echo", echo()).await;
        owner.tools_call("host.tool_share", json!({"name": name, "visibility": "org"})).await.expect("share as org");
    }
    // O keeps a third tool private; M has its own private `tt`.
    publish(&owner, "hidden", "echo", echo()).await;
    publish(&member, "tt", "echo", echo()).await;

    // A tenant outside the org, with a tool of its own shared publicly and one
    // it keeps private.
    let (x_ns, x_key) = signup(&server.base_url, "Outsider").await;
    let outsider = McpClient::with_bearer(&server.base_url, &x_key);
    publish(&outsider, "xx", "echo", echo()).await;
    publish(&outsider, "xpub", "echo", echo()).await;
    outsider.tools_call("host.tool_share", json!({"name": "xpub", "visibility": "public"})).await.expect("share public");

    let own_tt = format!("{member_ns}.tt");
    assert_eq!(listed(&member).await, sorted(vec!["org.tt".into(), "org.uu".into(), own_tt]), "member's list");
    let who = extract_structured(&member.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let via_org: Vec<(&str, &str)> = who["shared_tools"]
        .as_array()
        .expect("shared_tools")
        .iter()
        .filter(|t| t["via"] == "org")
        .map(|t| (t["name"].as_str().unwrap(), t["owner"].as_str().unwrap()))
        .collect();
    assert_eq!(via_org, vec![("org.tt", owner_ns.as_str()), ("org.uu", owner_ns.as_str())], "{who}");

    // The owner does not see its own tools renamed, and X sees no org name.
    assert_eq!(
        listed(&owner).await,
        sorted(vec![format!("{owner_ns}.hidden"), format!("{owner_ns}.tt"), format!("{owner_ns}.uu")]),
        "owner's list"
    );
    let outside = listed(&outsider).await;
    assert!(outside.iter().all(|n| !n.starts_with("org.")), "outsider lists no org tool: {outside:?}");
    assert_eq!(outside, sorted(vec![format!("{x_ns}.xpub"), format!("{x_ns}.xx")]), "outsider's list");
}
