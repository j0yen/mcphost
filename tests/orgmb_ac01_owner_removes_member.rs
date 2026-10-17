//! PRD-mcphost-org-model-b
//! AC1 — Given a member, When the owner calls `host.org.remove {member}`,
//! Then the member's next `host.whoami` has `org: null`, its tools and key
//! still work, and a `tenant_audit` row `org.remove` exists on the owner's
//! tenant; Given a non-owner caller, Then `not_owner`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn orgmb_ac01() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(
        &owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"),
    );
    let code = created["invite_url"]
        .as_str()
        .expect("invite_url")
        .trim_end_matches("/mcp")
        .rsplit('/')
        .next()
        .expect("code")
        .to_string();

    // An existing tenant with a tool joins with its own key.
    let (member_ns, member_key) = signup(&server.base_url, "Member").await;
    let member = McpClient::with_bearer(&server.base_url, &member_key);
    publish(
        &member,
        "alpha",
        "echo",
        json!({"schema": {"type": "object", "properties": {}, "required": []}}),
    )
    .await;
    member.tools_call("host.org.join", json!({"code": code})).await.expect("join");
    let who = extract_structured(&member.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(who["org"]["role"], json!("member"), "{who}");

    // A non-owner cannot remove anyone.
    let err = member
        .tools_call("host.org.remove", json!({"member": owner_ns}))
        .await
        .expect_err("member must not remove");
    assert_eq!(err.error_code.as_deref(), Some("not_owner"), "{err:?}");

    // The owner removes the member.
    let removed = extract_structured(
        &owner
            .tools_call("host.org.remove", json!({"member": member_ns}))
            .await
            .expect("owner removes member"),
    );
    assert_eq!(removed["removed"], json!(member_ns), "{removed}");

    let who = extract_structured(&member.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert!(who["org"].is_null(), "removed member's org must be null: {who}");
    let list = member.tools_list().await.expect("member key still works");
    assert!(
        list["tools"].as_array().expect("tools").iter().any(|t| t["name"].as_str().is_some_and(|n| n.contains("alpha"))),
        "member's own tool must survive removal: {list}"
    );

    let owner_tenant = server
        .state
        .db
        .find_tenant_by_namespace(owner_ns.clone())
        .await
        .expect("lookup")
        .expect("owner tenant");
    let rows = server
        .state
        .db
        .list_tenant_audit_for_subject(owner_tenant.id, member_ns.clone(), None, 10)
        .await
        .expect("audit");
    assert!(rows.iter().any(|(_, action, _)| action == "org.remove"), "audit rows: {rows:?}");
}
