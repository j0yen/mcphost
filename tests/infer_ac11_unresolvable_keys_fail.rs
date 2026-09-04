//! PRD-mcphost-tool-infer AC11 (P0, shared with AC12) — Given a source
//! that reads `args` but from which no key can be inferred (a dynamic
//! subscript, not a literal string), When it is published without
//! `args_schema`, Then publish fails with a structured error naming what
//! could not be inferred.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn dynamic_subscript_with_no_literal_key_fails_publish() {
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
    let (_ns, key) = signup(&server.base_url, "AC11 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // `args[key]` -- a genuine access attempt, but not a literal string key
    // inference can resolve.
    let spec = json!({
        "source": "def main(args):\n    key = \"city\"\n    return args[key]\n",
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "unresolvable", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("a dynamic-key access with no literal key must fail publish");

    // AC12: a stable, machine-readable code.
    assert_eq!(err.error_code.as_deref(), Some("args_schema_not_inferable"));
}

#[tokio::test]
async fn a_source_that_never_touches_args_is_not_an_inference_error() {
    // The negative case that keeps AC11 honest: a tool that simply doesn't
    // read `args` at all must still publish (with a permissive schema),
    // not be swept up by the same check.
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
    let (_ns, key) = signup(&server.base_url, "AC11b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "no_args_use", "kind": "python", "spec": spec}),
        )
        .await
        .expect("a tool that never reads args must publish fine with a permissive schema");
}
