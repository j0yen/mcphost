//! PRD-mcphost-sandbox-timeout-clamp-visible
//! AC2 (P1) -- Given a free tenant, When it publishes a python spec with
//! `timeout_s: 45`, Then the result carries exactly one `timeout_clamped`
//! warning naming field, requested, ceiling, plan; the same publish as a pro
//! tenant has no `warnings` key.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

fn spec() -> serde_json::Value {
    json!({"source": "def main(args):\n    return {}\n", "args_schema": {"type": "object"}, "timeout_s": 45})
}

#[tokio::test]
async fn stcv_ac02() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "STCV AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let published = client
        .tools_call("host.tool_publish", json!({"name": "slow", "kind": "python", "spec": spec()}))
        .await
        .expect("free publish with timeout_s 45 is accepted");
    let out = extract_structured(&published);
    let warnings = out["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 1, "{out}");
    let w = &warnings[0];
    assert_eq!(w["code"], json!("timeout_clamped"));
    assert_eq!(w["field"], json!("timeout_s"));
    assert_eq!(w["requested"], json!(45));
    assert_eq!(w["ceiling"], json!(30));
    assert_eq!(w["ceiling_field"], json!("sandbox_cpu_s_max"));
    assert_eq!(w["effective"], json!(30));
    assert_eq!(w["plan"], json!("free"));
    let msg = w["message"].as_str().expect("message");
    for needle in ["timeout_s", "45", "30", "free"] {
        assert!(msg.contains(needle), "message {msg:?} lacks {needle}");
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.plan_set", json!({"tenant": ns, "plan": "pro", "reason": "STCV AC2 setup"}))
        .await
        .expect("admin.plan_set to pro");
    let pro = client
        .tools_call("host.tool_publish", json!({"name": "slow", "kind": "python", "spec": spec()}))
        .await
        .expect("pro publish");
    assert!(extract_structured(&pro).get("warnings").is_none(), "pro has no warnings key");
}
