//! PRD-mcphost-first-publish-real-kind
//! AC2 (P0) -- Given the sandbox pool reports a 7 s wait, When a python
//! publish hits `sandbox_unavailable`, Then `data.retry_after_s == 7`,
//! `data.alternatives == ["http"]`, and the message text contains "7".

use crate::common;
use common::{fake_interpreter_failing, python_kind_registry_with_selftest, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn sandbox_unavailable_reports_the_pools_own_retry_estimate() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.run_startup_selftest().await;
    // Simulate "the sandbox pool reports a 7 s wait" -- this crate does not
    // track real queue depth (PRD non-goal: sandbox capacity), so the
    // estimate is injected directly for this test.
    py.set_queue_wait_estimate_for_test(Some(7));

    let server = common::TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "my_py",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {}\n"},
            }),
        )
        .await
        .expect_err("publish on an unready sandbox must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("sandbox_unavailable"));
    assert_eq!(err.data["retry_after_s"], json!(7));
    assert_eq!(err.data["alternatives"], json!(["http"]));
    assert!(
        err.message.contains('7'),
        "message must name the retry seconds: {}",
        err.message
    );
}
