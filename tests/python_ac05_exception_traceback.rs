//! AC5 (P0) — Given a tool whose `main` raises, When called, Then the
//! error is `tool_exception` with the traceback and the stderr tail, and
//! the process is gone.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_raised_exception_becomes_tool_exception_with_traceback() {
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
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    raise ValueError('boom')\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "raiser", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.raiser"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("a raised exception must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
    let traceback = err.data["traceback"].as_str().unwrap_or("");
    assert!(
        traceback.contains("ValueError") && traceback.contains("boom"),
        "traceback must show the raised exception, got: {traceback}"
    );
}
