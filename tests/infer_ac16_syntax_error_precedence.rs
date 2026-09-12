//! PRD-mcphost-tool-infer AC16 (P0) — Given a source that is not valid
//! Python, When it is published without `args_schema`, Then the existing
//! spec-validation error is returned, not an inference error.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn syntax_error_wins_over_any_inference_error() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC16 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Unparseable AND, if it somehow parsed, would also trip the
    // unresolvable-key inference error -- proving syntax-checking runs and
    // wins first.
    let spec = json!({
        "source": "def main(args):\n    return args[\n",
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_syntax", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("unparseable source must be rejected");

    assert!(
        err.message.contains("source") && err.message.to_ascii_lowercase().contains("line"),
        "must be the existing syntax-validation error naming source and a line, got: {}",
        err.message
    );
    assert_ne!(
        err.error_code.as_deref(),
        Some("args_schema_not_inferable"),
        "a syntax error must never surface as an inference error"
    );
    assert_ne!(
        err.error_code.as_deref(),
        Some("requirement_not_inferable"),
        "a syntax error must never surface as an inference error"
    );
}
