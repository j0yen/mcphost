//! PRD-mcphost-python-kind-plain-env AC4 (P0) — Given a tool with secret
//! `API_KEY` set, When a publish declares env `API_KEY`, Then it is refused
//! naming the collision; Given a tool published with env `MODE`, When
//! `secret_set MODE` is attempted, Then it is refused symmetrically.
//!
//! Both directions publish at least one python tool with a real (trivial)
//! `source`, which reaches `Kind::validate_async`'s sandboxed AST check
//! before the collision itself is ever evaluated (`control::tool_publish`
//! runs `validate_async` before the env/secret collision check) -- so,
//! unlike AC2/AC3's refusal cases, this needs the sandbox skip guard.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn publishing_env_that_collides_with_an_existing_secret_is_refused() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC4a Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "API_KEY", "value": "sekret"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {"API_KEY": "not-a-secret"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "colliding_env", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("env colliding with an existing secret name must be refused");
    assert_eq!(err.error_code.as_deref(), Some("env_secret_collision"));
    assert_eq!(err.data["key"], json!("API_KEY"));
}

#[tokio::test]
async fn setting_a_secret_that_collides_with_a_published_env_entry_is_refused() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC4b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {"MODE": "fast"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "with_mode_env", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call("host.secret_set", json!({"name": "MODE", "value": "v"}))
        .await
        .expect_err("a secret colliding with a published env entry must be refused");
    assert_eq!(err.error_code.as_deref(), Some("env_secret_collision"));
    assert_eq!(err.data["key"], json!("MODE"));
}
