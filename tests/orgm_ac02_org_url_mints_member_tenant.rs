//! PRD-mcphost-org-model
//! AC2 — Given a new client connecting to `/o/<code>/mcp`, When it calls
//! `host.whoami`, Then it has its own `namespace`, its own `/u/` URL, and
//! `org: {name, role: "member"}`; the owner's `host.org.members` lists it
//! within the same request.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn orgm_ac02_org_url_mints_own_tenant_and_lists_it_for_the_owner() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(
        &owner
            .tools_call("host.org.create", json!({"name": "acme"}))
            .await
            .expect("host.org.create"),
    );
    let org_path = created["invite_url"]
        .as_str()
        .expect("invite_url")
        .trim_start_matches(&server.base_url)
        .to_string();
    assert!(org_path.starts_with("/o/") && org_path.ends_with("/mcp"), "{org_path}");

    let newcomer = McpClient::new(&server.base_url).with_path(&org_path).with_session_continuity();
    let whoami = extract_structured(
        &newcomer.tools_call("host.whoami", json!({})).await.expect("first call on the org URL"),
    );

    let ns = whoami["namespace"].as_str().expect("namespace");
    assert_ne!(ns, owner_ns, "the newcomer must get its own namespace: {whoami}");
    let own_url = whoami["onboarding"]["url"].as_str().expect("onboarding.url");
    let own_path = own_url.trim_start_matches(&server.base_url);
    assert!(
        own_path.starts_with("/u/") && own_path.ends_with("/mcp"),
        "its own /u/ URL, got {own_url}"
    );
    assert_eq!(whoami["org"]["name"], json!("acme"), "{whoami}");
    assert_eq!(whoami["org"]["role"], json!("member"), "{whoami}");

    // The minted /u/ URL is that same tenant.
    let via_url = McpClient::new(&server.base_url).with_path(own_path);
    let again = extract_structured(&via_url.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(again["namespace"], whoami["namespace"], "{again}");
    assert_eq!(again["org"]["role"], json!("member"), "{again}");

    // Same request window: the owner's very next listing already has it.
    let members = extract_structured(
        &owner.tools_call("host.org.members", json!({})).await.expect("host.org.members"),
    );
    let rows = members["members"].as_array().expect("members array");
    let listed = rows
        .iter()
        .find(|m| m["namespace"] == json!(ns))
        .unwrap_or_else(|| panic!("owner's members lacks {ns}: {members}"));
    assert_eq!(listed["role"], json!("member"), "{members}");
    assert!(rows.iter().any(|m| m["namespace"] == json!(owner_ns) && m["role"] == json!("owner")));
}
