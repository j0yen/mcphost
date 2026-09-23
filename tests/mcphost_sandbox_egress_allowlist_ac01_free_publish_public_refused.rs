//! PRD-mcphost-sandbox-egress-allowlist
//! AC1 (P0) — Given a Free tenant, When it publishes a python tool with
//! `network: "public"`, Then `plan_required` naming `pro` and `network`,
//! and no tool row is created.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn free_tenant_publish_network_public_is_refused() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC1 Free Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "network": "public",
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "downloader", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("a free tenant publishing network: public must be refused");

    assert_eq!(err.error_code.as_deref(), Some("plan_required"));
    assert_eq!(err.data["plan"], json!("pro"));
    assert_eq!(err.data["field"], json!("network"));

    let tools = extract_structured(
        &client.tools_call("host.tool_list", json!({})).await.expect("tool_list"),
    );
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    assert!(!names.contains(&"downloader"), "no tool row must exist: {names:?}");
}
