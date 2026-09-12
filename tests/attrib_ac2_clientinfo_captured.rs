//! PRD-mcphost-tenant-attribution
//! AC2 — Given a signup whose `initialize` carried `clientInfo
//! {name:"claude-code", version:"2.1"}`, When it completes, Then the
//! tenant row stores that client name and version and `tenants_by_client`
//! on `/healthz` counts it.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn clientinfo_is_captured_and_counted() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("claude-code", "2.1");

    // Narrative fidelity with AC2's wording -- functionally the signup
    // call below carries its own `_meta["io.modelcontextprotocol/clientInfo"]`
    // too (this host runs every call stateless; see `peer_client_info`'s
    // doc comment in `src/handler.rs`), so this isn't load-bearing for the
    // assertions below, but it is what a real client actually does.
    client.initialize().await;

    let signup_result = client
        .tools_call("signup", json!({"name": "Claude Code User"}))
        .await
        .expect("signup");
    let ns = extract_structured(&signup_result)["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.client_name.as_deref(), Some("claude-code"));
    assert_eq!(tenant.client_version.as_deref(), Some("2.1"));

    let health = healthz(&server.base_url).await;
    let by_client = health["tenants_by_client"].as_array().expect("tenants_by_client array");
    let row = by_client
        .iter()
        .find(|r| r["client_name"] == json!("claude-code"))
        .unwrap_or_else(|| panic!("claude-code missing from tenants_by_client: {by_client:?}"));
    assert_eq!(row["count"], json!(1), "{row:?}");
}
