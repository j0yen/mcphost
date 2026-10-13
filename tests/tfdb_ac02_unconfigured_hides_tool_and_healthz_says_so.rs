//! AC2 (P0) -- Given no `MCPHOST_DRAFT_API_URL`, When `tools/list` runs
//! anonymous and keyed, Then `host.tool_from_description` is absent from
//! both, a direct call returns `draft_unavailable` with
//! `next_call.tool == "host.quickstart"`, and `/healthz` says
//! `draft_configured: false`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, python_kind_registry, signup};
use mcphost::drafter::FakeDrafter;
use serde_json::{Value, json};
use std::sync::Arc;

const TOOL: &str = "host.tool_from_description";

fn lists(listing: &Value) -> bool {
    listing["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .any(|t| t["name"] == json!(TOOL))
}

async fn healthz(base_url: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("healthz json")
}

#[tokio::test]
async fn unconfigured_host_hides_the_tool_refuses_calls_and_reports_it() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TFDB AC2 Tenant").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let anonymous = McpClient::new(&server.base_url);

    assert!(!lists(&anonymous.tools_list().await.expect("anonymous tools/list")));
    assert!(!lists(&keyed.tools_list().await.expect("keyed tools/list")));

    let e = keyed
        .tools_call(TOOL, json!({"description": "return the current UTC time as ISO 8601"}))
        .await
        .expect_err("no drafter configured");
    assert_eq!(e.error_code.as_deref(), Some("draft_unavailable"), "{} {}", e.code, e.message);
    assert_eq!(e.data["next_call"]["tool"], json!("host.quickstart"), "{}", e.data);

    let health = healthz(&server.base_url).await;
    assert_eq!(health["draft_configured"], json!(false), "{health}");
    assert_eq!(health["drafts_24h"]["n"], json!(0), "{health}");
}

#[tokio::test]
async fn configured_host_lists_the_tool_for_anonymous_and_keyed() {
    let envs_dir = common::TempDataDir::new();
    let fake = Arc::new(FakeDrafter::new(vec![]));
    let server = TestServer::start_with_drafter(python_kind_registry(&envs_dir.0), fake).await;
    let (_ns, key) = signup(&server.base_url, "TFDB AC2 Configured").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let anonymous = McpClient::new(&server.base_url);
    assert!(lists(&anonymous.tools_list().await.expect("anonymous tools/list")));
    assert!(lists(&keyed.tools_list().await.expect("keyed tools/list")));
}
