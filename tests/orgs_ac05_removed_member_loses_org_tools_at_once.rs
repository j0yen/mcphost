//! PRD-mcphost-org-shared-tools-namespace-v2
//! AC5 — Given M removed by `host.org.remove`, When M calls `org.t` in the
//! next request, Then `org_required` with `data.next_call`
//! `host.org.create` (a live member of another org calling `org.t` gets
//! `shared_tool_not_found`), and M's `tools/list` omits `org.t`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::{Value, json};

fn echo() -> Value {
    json!({"schema": {"type": "object"}})
}

async fn org_code(owner: &McpClient, name: &str) -> String {
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": name})).await.expect("create"));
    created["invite_url"]
        .as_str()
        .expect("invite_url")
        .trim_end_matches("/mcp")
        .rsplit('/')
        .next()
        .expect("code")
        .to_string()
}

async fn lists_org_tool(client: &McpClient, name: &str) -> bool {
    let list = client.tools_list().await.expect("tools/list");
    list["tools"].as_array().expect("tools").iter().any(|t| t["name"] == name)
}

#[tokio::test]
async fn orgs_ac05() {
    let server = TestServer::start().await;
    let (_owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let code = org_code(&owner, "acme").await;
    let (member_ns, member_key) = signup(&server.base_url, "Member").await;
    let member = McpClient::with_bearer(&server.base_url, &member_key);
    member.tools_call("host.org.join", json!({"code": code})).await.expect("join");
    let (pub_ns, pub_key) = signup(&server.base_url, "Publisher").await;
    let publisher = McpClient::with_bearer(&server.base_url, &pub_key);
    publisher.tools_call("host.org.join", json!({"code": code})).await.expect("join");

    publish(&owner, "tt", "echo", echo()).await;
    owner.tools_call("host.tool_share", json!({"name": "tt", "visibility": "org"})).await.expect("share tt");
    publish(&publisher, "pp", "echo", echo()).await;
    publisher.tools_call("host.tool_share", json!({"name": "pp", "visibility": "org"})).await.expect("share pp");

    // While live, M calls both and lists both.
    member.tools_call("org.tt", json!({"msg": "hi"})).await.expect("live member calls org.tt");
    assert!(lists_org_tool(&member, "org.tt").await);
    assert!(lists_org_tool(&member, "org.pp").await);

    // A live member of ANOTHER org gets the plain not-found, never a hint at
    // the first org.
    let (_other_ns, other_key) = signup(&server.base_url, "Other Owner").await;
    let other = McpClient::with_bearer(&server.base_url, &other_key);
    org_code(&other, "elsewhere").await;
    let err = other.tools_call("org.tt", json!({})).await.expect_err("another org's member");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert!(err.data.get("next_call").is_none_or(|n| n != "host.org.create"), "{:?}", err.data);

    // The publisher leaves the org: M's next call to its tool is the same
    // not-found, and the entry is gone from M's list.
    owner.tools_call("host.org.remove", json!({"member": pub_ns})).await.expect("remove publisher");
    let err = member.tools_call("org.pp", json!({})).await.expect_err("publisher gone");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert!(!lists_org_tool(&member, "org.pp").await);

    // M is removed: the very next call is `org_required`, and the list omits it.
    owner.tools_call("host.org.remove", json!({"member": member_ns})).await.expect("remove member");
    let err = member.tools_call("org.tt", json!({})).await.expect_err("removed member");
    assert_eq!(err.error_code.as_deref(), Some("org_required"), "{err:?}");
    assert_eq!(err.data["next_call"], json!("host.org.create"), "{:?}", err.data);
    assert!(!lists_org_tool(&member, "org.tt").await, "removed member lists no org tool");
}
