//! AC8 (P0) — Given a tool with `network: none` that opens a TCP socket to
//! a public host, When called, Then the connection fails inside the
//! sandbox and the result is `tool_exception`, not a successful call.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn network_none_blocks_an_outbound_connection() {
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
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import socket\ndef main(args):\n    s = socket.create_connection(('1.1.1.1', 80), timeout=3)\n    s.close()\n    return {\"connected\": True}\n",
        "args_schema": {"type": "object"},
        "network": "none",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dialer", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.dialer"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("a connection attempt under network:none must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
}
