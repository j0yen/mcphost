//! PRD-mcphost-org-cost-attribution
//! AC2 — Given a member caller, When it calls `host.org.usage`, Then the
//! response holds its own row, `totals`, and no other member's namespace.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn orgc_ac02() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"));
    let code = created["invite_url"].as_str().expect("url").trim_end_matches("/mcp").rsplit('/').next().expect("code").to_string();
    let (member_ns, member_key) = signup(&server.base_url, "Ann").await;
    let member = McpClient::with_bearer(&server.base_url, &member_key);
    member.tools_call("host.org.join", json!({"code": code})).await.expect("join");

    let tenant = server.state.db.find_tenant_by_namespace(owner_ns.clone()).await.expect("lookup").expect("owner");
    for _ in 0..4 {
        server
            .state
            .db
            .record_call(tenant.id, "t".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("record_call");
    }

    let got = extract_structured(&member.tools_call("host.org.usage", json!({"window": "24h"})).await.expect("host.org.usage"));
    let rows = got["members"].as_array().expect("members");
    assert_eq!(rows.len(), 1, "{got}");
    assert_eq!(rows[0]["namespace"], json!(member_ns), "{got}");
    assert!(got["totals"].is_object(), "{got}");
    assert_eq!(got["totals"]["calls"], json!(4), "totals cover the whole org: {got}");
    assert!(!got.to_string().contains(&owner_ns), "no other member's namespace may appear: {got}");
}
