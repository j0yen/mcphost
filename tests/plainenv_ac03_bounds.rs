//! PRD-mcphost-python-kind-plain-env AC3 (P0) — Given an env map with 17
//! entries, and another totaling over 4 KiB, When published, Then each is
//! refused with a structured error naming the bound; Given exactly 16
//! entries under 4 KiB, Then publish succeeds.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

fn n_entries(n: usize) -> serde_json::Map<String, serde_json::Value> {
    (0..n)
        .map(|i| (format!("VAR{i}"), json!("v")))
        .collect()
}

/// The refusal cases below never reach the sandbox (same reasoning as
/// tests/plainenv_ac02_invalid_names_refused.rs: bounds are checked in
/// `validate_spec_fields_all`, synchronous and run before `validate_async`),
/// so they need no CI skip guard.
#[tokio::test]
async fn seventeen_entries_is_refused_naming_the_max_entries_bound() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3a Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": n_entries(17),
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "too_many", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("17 env entries must be refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["rule"], json!("max_entries"));
    assert_eq!(err.data["limit"], json!(16));
}

#[tokio::test]
async fn over_4kib_total_is_refused_naming_the_max_total_bytes_bound() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // One entry, well over 4 KiB by itself.
    let big_value = "x".repeat(5 * 1024);
    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {"BIG": big_value},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "too_big", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("over 4 KiB of env must be refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert_eq!(err.data["rule"], json!("max_total_bytes"));
    assert_eq!(err.data["limit"], json!(4096));
}

/// This sub-test publishes a spec that passes every synchronous check, so
/// it DOES reach `validate_async`'s sandboxed AST check -- needs the same
/// skip guard as tests/plainenv_ac01_env_reaches_process.rs.
#[tokio::test]
async fn exactly_sixteen_entries_under_4kib_publishes_successfully() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3c Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": n_entries(16),
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "at_bounds", "kind": "python", "spec": spec}),
        )
        .await
        .expect("exactly 16 small entries must publish successfully");
}
