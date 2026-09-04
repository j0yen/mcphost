//! AC11 (P0) — Given a tool listing `secrets: ["token"]`, When called, Then
//! `SECRET_TOKEN` is present in the sandbox, absent from a tool that does
//! not list it, and the value appears in no log, result or error.
//!
//! The tool's own result deliberately echoes the raw secret value it read
//! from the environment, to prove `kinds::python::redact_value` catches it
//! (requirement 7: redacted "from any string that leaves the host" -- a
//! tool's result is exactly such a string, not just an error/traceback).

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const SECRET_VALUE: &str = "sekret-value-should-never-leave-9f2a";

#[tokio::test]
async fn secret_present_and_redacted_when_declared() {
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
    let (ns, key) = signup(&server.base_url, "AC11a Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "token", "value": SECRET_VALUE}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "source": "import os\ndef main(args):\n    v = os.environ.get('SECRET_TOKEN')\n    return {\"present\": v is not None, \"echoed\": v}\n",
        "args_schema": {"type": "object"},
        "secrets": ["token"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "leaker", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.leaker"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(
        structured["present"],
        json!(true),
        "SECRET_TOKEN must be present when 'token' is declared in secrets"
    );
    let echoed = structured["echoed"].as_str().unwrap_or("");
    assert_ne!(
        echoed, SECRET_VALUE,
        "the raw secret value must never appear verbatim in a result that leaves the host"
    );
    assert!(
        echoed.contains("***"),
        "the redacted echo should show the '***' marker, got: {echoed}"
    );
}

#[tokio::test]
async fn secret_absent_when_not_declared() {
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
    let (ns, key) = signup(&server.base_url, "AC11b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "token", "value": SECRET_VALUE}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "source": "import os\ndef main(args):\n    return {\"present\": os.environ.get('SECRET_TOKEN') is not None}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "no_secret", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.no_secret"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&result)["present"], json!(false));
}
