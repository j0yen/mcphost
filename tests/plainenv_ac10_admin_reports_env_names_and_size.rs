//! PRD-mcphost-python-kind-plain-env AC10 (P2, best effort) — Given an
//! admin inspection of a tool with env, When invoked, Then it reports env
//! names and total size, never values.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn admin_tool_list_reports_env_names_and_size_never_values() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    const SECRET_LOOKING_VALUE: &str = "should-never-be-reported-3c1e";
    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {"MODE": SECRET_LOOKING_VALUE},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "inspected", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = admin
        .tools_call("admin.tool_list", json!({"tenant": ns}))
        .await
        .expect("admin.tool_list ok");
    let structured = common::extract_structured(&result);
    let tools = structured["tools"].as_array().expect("tools array");
    let tool = tools
        .iter()
        .find(|t| t["name"] == json!(format!("{ns}.inspected")))
        .expect("published tool must be listed");

    assert_eq!(tool["env_names"], json!(["MODE"]));
    assert_eq!(tool["env_total_bytes"], json!("MODE".len() + SECRET_LOOKING_VALUE.len()));

    let rendered = tool.to_string();
    assert!(
        !rendered.contains(SECRET_LOOKING_VALUE),
        "admin.tool_list must never report an env value: {rendered}"
    );
}
