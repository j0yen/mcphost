//! PRD-mcphost-end-user-identity
//! AC4 (P0) — Given a call carrying no end-user identity (a plain
//! key-based call, no `end_user_assertion`), When a python-kind tool runs,
//! Then `MCPHOST_END_USER_ID` is UNSET in its environment (not empty), and
//! the `calls` row's end-user columns are all null.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn no_identity_leaves_python_env_unset_and_calls_row_null() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\n\
            def main(args):\n\
            \treturn {\n\
            \t    \"has_id\": \"MCPHOST_END_USER_ID\" in os.environ,\n\
            \t    \"has_issuer\": \"MCPHOST_END_USER_ISSUER\" in os.environ,\n\
            \t    \"has_method\": \"MCPHOST_END_USER_METHOD\" in os.environ,\n\
            \t}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "whoami_tool", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // No OAuth bearer, no end_user_assertion argument -- an ordinary
    // key-based call carrying no identity at all.
    let result = poll_until_ready(
        &client,
        &format!("{ns}.whoami_tool"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["has_id"], json!(false), "MCPHOST_END_USER_ID must be unset: {structured}");
    assert_eq!(structured["has_issuer"], json!(false), "{structured}");
    assert_eq!(structured["has_method"], json!(false), "{structured}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let (subject, issuer_col, method) = server
        .state
        .db
        .last_call_end_user_for_test(tenant.id, "whoami_tool".to_string())
        .await
        .unwrap()
        .expect("a calls row must exist for whoami_tool");
    assert_eq!(subject, None, "calls.end_user_subject must be null");
    assert_eq!(issuer_col, None, "calls.end_user_issuer must be null");
    assert_eq!(method, None, "calls.end_user_method must be null");
}
