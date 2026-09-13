//! PRD-mcphost-python-kind-plain-env AC2 (P0) — Given env names `MCPHOST_X`,
//! `PATH`, and `lower_case`, When each is published, Then each publish is
//! refused with a structured error naming the key and the violated rule.
//!
//! None of these reach the sandbox: `kinds::python::validate_spec_fields_all`
//! (called from `Kind::validate_all`, a synchronous check) rejects them
//! before `host.tool_publish` ever calls `Kind::validate_async` (the
//! sandboxed AST check) -- so, unlike AC1/AC4/AC5/AC6/AC8, this test needs
//! no `sandbox::require_user_namespaces_or_ci_skip()` guard.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use serde_json::json;

async fn publish_with_env(name: &str, env_key: &str) -> common::RpcError {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {env_key: "v"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "python", "spec": spec}),
        )
        .await
        .expect_err(&format!("env key '{env_key}' must be rejected"))
}

#[tokio::test]
async fn mcphost_prefixed_name_is_rejected_naming_the_key_and_rule() {
    let err = publish_with_env("bad_env_mcphost", "MCPHOST_X").await;
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["key"], json!("MCPHOST_X"));
    assert_eq!(err.data["rule"], json!("reserved_name"));
}

#[tokio::test]
async fn path_is_rejected_as_a_reserved_exact_name() {
    let err = publish_with_env("bad_env_path", "PATH").await;
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["key"], json!("PATH"));
    assert_eq!(err.data["rule"], json!("reserved_name"));
}

#[tokio::test]
async fn lower_case_name_is_rejected_for_the_name_pattern() {
    let err = publish_with_env("bad_env_lower", "lower_case").await;
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["key"], json!("lower_case"));
    assert_eq!(err.data["rule"], json!("name_pattern"));
}
