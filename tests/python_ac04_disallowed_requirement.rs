//! AC4 (P0) — Given a requirement with a URL or path, When published, Then
//! it is rejected with `requirement_not_allowed`.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

async fn publish_with_requirement(name: &str, requirement: &str) -> common::RpcError {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "requirements": [requirement],
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "python", "spec": spec}),
        )
        .await
        .expect_err(&format!("requirement '{requirement}' must be rejected"))
}

// Requirement 8/9: `publish_with_requirement` above builds and runs a real
// python-kind tool via the sandbox, which needs unprivileged user
// namespaces. Not guaranteed on GitHub's hosted runners, so each test below
// skips cleanly rather than fail with "the tool's environment failed to
// build" -- same pattern as the sandbox-dependent unit tests in
// src/kinds/python.rs and src/sandbox.rs, and as
// tests/ac17_kind_conformance.rs. (The guard lives in each #[tokio::test]
// rather than in the helper itself, since the helper's return type is
// `common::RpcError`, not `()`.)

#[tokio::test]
async fn url_requirement_is_rejected() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let err = publish_with_requirement("bad_req_url", "pkg @ https://example.com/pkg.whl").await;
    assert_eq!(err.error_code.as_deref(), Some("requirement_not_allowed"));
}

#[tokio::test]
async fn vcs_requirement_is_rejected() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let err = publish_with_requirement("bad_req_vcs", "git+https://github.com/example/pkg").await;
    assert_eq!(err.error_code.as_deref(), Some("requirement_not_allowed"));
}

#[tokio::test]
async fn path_requirement_is_rejected() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let err = publish_with_requirement("bad_req_path", "../local-package").await;
    assert_eq!(err.error_code.as_deref(), Some("requirement_not_allowed"));
}
