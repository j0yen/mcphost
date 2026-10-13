//! AC3 (P0) -- Given an anonymous call on `/mcp` with only `description`,
//! When it completes, Then the response carries the implicit-signup
//! `onboarding` envelope and the published tool belongs to the new tenant.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry};
use mcphost::drafter::{Draft, FakeDrafter};
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn anonymous_description_call_signs_up_and_publishes_under_the_new_tenant() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let good = Draft {
        name: Some("doubler".into()),
        spec: json!({"source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n"}),
        invocations: vec![json!({"n": 3})],
        ..Draft::default()
    };
    let fake = Arc::new(FakeDrafter::new(vec![good]));
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_drafter(python_kind_registry(&envs_dir.0), fake).await;

    // No header, no `tenant_key`: the literal first call of a brand-new agent.
    let session = McpClient::new(&server.base_url).with_session_continuity();
    let result = session
        .tools_call("host.tool_from_description", json!({"description": "double a number"}))
        .await
        .expect("an anonymous description call must succeed, not fail tenant_key_missing");
    let out = extract_structured(&result);

    let onboarding = &out["onboarding"];
    let tenant = onboarding["tenant"].as_str().expect("onboarding.tenant");
    let url = onboarding["url"].as_str().expect("onboarding.url");
    assert!(url.contains("/u/") && url.ends_with("/mcp"), "onboarding.url {url}");

    assert_eq!(out["test"]["ok"], json!(true), "{out}");
    assert_eq!(out["qualified_name"], json!(format!("{tenant}.doubler")), "{out}");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "exactly one tenant is created");
    assert_eq!(tenants[0].namespace, tenant);
    let rows = server.state.db.list_tools(tenants[0].id).await.expect("list tools");
    assert!(rows.iter().any(|r| r.name == "doubler"), "the published tool belongs to the new tenant");
}
