//! AC3 (P0) — Given source that does not define `main` or does not parse,
//! When published, Then the publish is rejected with the error naming
//! `source` and, for a syntax error, the line number.
//!
//! The check itself runs in the sandbox, never in the host process
//! (requirement 2) — this test exercises the real publish path
//! (`host.tool_publish` -> `Kind::validate_async` -> `kinds::sandbox::run`),
//! not `kinds::python`'s unit tests, which call `validate_async` directly.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn missing_main_is_rejected_naming_source() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3a Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def not_main(args):\n    return args\n",
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("publish of source without main() must be rejected");
    assert!(
        err.message.contains("source"),
        "error must name 'source', got: {}",
        err.message
    );
}

#[tokio::test]
async fn syntax_error_is_rejected_naming_the_line() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return (\n",
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_syntax", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("publish of unparseable source must be rejected");
    assert!(
        err.message.contains("source") && err.message.to_ascii_lowercase().contains("line"),
        "error must name 'source' and a line number, got: {}",
        err.message
    );
}
