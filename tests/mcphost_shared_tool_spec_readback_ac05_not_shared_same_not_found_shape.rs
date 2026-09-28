//! PRD-mcphost-shared-tool-spec-readback
//! AC5 (P0) — Given a tool not shared with the caller, When it calls
//! `host.tool_spec_shared`, Then the response is the same not-found shape
//! `host.tool_call` returns for an unshared tool.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn caller_not_shared_with_gets_tool_not_found_like_tool_call() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_o, key_o) = signup(&server.base_url, "AC5 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "private_tool", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok, never shared");

    let (_ns_s, key_s) = signup(&server.base_url, "AC5 Stranger").await;
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);
    let qualified = format!("{ns_o}.private_tool");

    let call_err = client_s
        .tools_call("host.tool_call", json!({"name": qualified.clone(), "args": {}}))
        .await
        .expect_err("host.tool_call on an unshared tool must be tool_not_found");
    assert_eq!(call_err.error_code.as_deref(), Some("tool_not_found"));

    let spec_err = client_s
        .tools_call("host.tool_spec_shared", json!({"tool": qualified}))
        .await
        .expect_err("host.tool_spec_shared on an unshared tool must be tool_not_found too");
    assert_eq!(spec_err.error_code.as_deref(), Some("tool_not_found"));
    assert_eq!(
        spec_err.message, call_err.message,
        "host.tool_spec_shared's not-found message must match host.tool_call's byte-for-byte"
    );
}
