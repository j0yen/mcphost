//! PRD-mcphost-org-model
//! AC4 — Given an org at `org_members_max`, When a third join arrives on
//! the org URL, Then the response is `org_members_max` with `limit`,
//! `plan`, and `upgrade` fields and no tenant is minted.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn orgm_ac04_third_join_at_free_cap_is_refused_and_mints_no_tenant() {
    let server = TestServer::start().await;
    let (_owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
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
    let cap = server
        .state
        .plans
        .get("free")
        .expect("free plan")
        .org_members_max;
    assert_eq!(cap, 3, "free org_members_max");

    // Owner + two joiners fill the free cap of 3.
    for i in 0..(cap - 1) {
        let joiner = McpClient::new(&server.base_url).with_path(&org_path).with_session_continuity();
        let who = extract_structured(
            &joiner
                .tools_call("host.whoami", json!({}))
                .await
                .unwrap_or_else(|e| panic!("join {i} should fit under the cap: {} {}", e.code, e.message)),
        );
        assert_eq!(who["org"]["role"], json!("member"), "{who}");
    }
    let tenants_before = server.state.db.list_tenants().await.expect("tenants").len();

    // The third join on the org URL is refused with the structured error.
    let third = McpClient::new(&server.base_url).with_path(&org_path).with_session_continuity();
    let err = third
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a join past org_members_max must be refused");
    assert_eq!(err.error_code.as_deref(), Some("org_members_max"), "{err:?}");
    assert_eq!(err.data["limit"], json!(cap), "{:?}", err.data);
    assert_eq!(err.data["plan"], json!("free"), "{:?}", err.data);
    assert!(
        err.data["upgrade"].as_str().is_some_and(|u| !u.is_empty()),
        "upgrade must name where to raise the cap: {:?}",
        err.data
    );

    let tenants_after = server.state.db.list_tenants().await.expect("tenants").len();
    assert_eq!(tenants_before, tenants_after, "a refused join must not mint a tenant");

    // The same refusal for an existing tenant calling host.org.join.
    let (_ns, key) = signup(&server.base_url, "Existing").await;
    let existing = McpClient::with_bearer(&server.base_url, &key);
    let code = org_path.trim_start_matches("/o/").trim_end_matches("/mcp").to_string();
    let err = existing
        .tools_call("host.org.join", json!({"code": code}))
        .await
        .expect_err("host.org.join past the cap must be refused");
    assert_eq!(err.error_code.as_deref(), Some("org_members_max"), "{err:?}");
}
