//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC3 (P0) -- Given a free tenant and a tool with `memory_mb: 512`, When it
//! is published, Then the result is `plan_required` naming
//! `sandbox_memory_mb_max` and `256`; the same spec on pro is accepted.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

fn spec(memory_mb: u64) -> serde_json::Value {
    json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "memory_mb": memory_mb,
    })
}

#[tokio::test]
async fn srlp_ac03_free_refuses_512mb_and_pro_accepts_it() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "SRLP AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_publish", json!({"name": "big", "kind": "python", "spec": spec(512)}))
        .await
        .expect_err("free tenant publishing memory_mb: 512 must be refused");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"));
    assert_eq!(err.data["field"], json!("sandbox_memory_mb_max"));
    assert_eq!(err.data["limit"], json!(256));
    assert_eq!(err.data["requested"], json!(512));
    assert!(err.message.contains("sandbox_memory_mb_max") && err.message.contains("256"), "{}", err.message);

    // At the ceiling is fine on free.
    client
        .tools_call("host.tool_publish", json!({"name": "fits", "kind": "python", "spec": spec(256)}))
        .await
        .expect("memory_mb equal to the free ceiling is accepted");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.plan_set", json!({"tenant": ns, "plan": "pro", "reason": "SRLP AC3 setup"}))
        .await
        .expect("admin.plan_set to pro");
    client
        .tools_call("host.tool_publish", json!({"name": "big", "kind": "python", "spec": spec(512)}))
        .await
        .expect("the same spec on pro is accepted");
}
