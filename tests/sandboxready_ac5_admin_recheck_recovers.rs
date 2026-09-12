//! PRD-mcphost-sandbox-ready
//! AC5 -- Given `sandbox_ready: false` and the underlying cause repaired
//! (test: swap the injected interpreter back to a working one), When
//! `admin.sandbox_recheck` is called with the admin key, Then it returns
//! `ready: true`, `/healthz` reports `sandbox_ready: true` without a
//! restart, and a subsequent python-kind publish succeeds.

use crate::common;
use common::{ADMIN_KEY, fake_interpreter_failing, python_kind_registry_with_selftest, signup};
use mcphost::sandbox;
use serde_json::json;

// PRD-mcphost-healthz-minimal: `sandbox_ready` moved behind the admin
// bearer -- the anonymous body is just `{"ok": true/false}` now.
async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn admin_sandbox_recheck_flips_unready_to_ready_without_a_restart() {
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

    let server = common::TestServer::start_with_kinds(kinds).await;
    let before = healthz(&server.base_url).await;
    assert_eq!(before["sandbox_ready"], json!(false), "healthz: {before}");

    // "the underlying cause repaired": swap the injected interpreter back
    // to the real one.
    py.set_selftest_interpreter_for_test(None);

    let admin = common::McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.sandbox_recheck", json!({}))
        .await
        .expect("admin.sandbox_recheck");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["ready"], json!(true), "result: {structured}");

    let after = healthz(&server.base_url).await;
    assert_eq!(
        after["sandbox_ready"],
        json!(true),
        "healthz must reflect the recheck without a restart: {after}"
    );

    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "my_py",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {\"ok\": True}\n"},
            }),
        )
        .await
        .expect("a python publish must succeed once the recheck reports ready");
}

/// A tenant key must never reach `admin.sandbox_recheck` -- same admin-only
/// gate every other `admin.*` tool has.
#[tokio::test]
async fn tenant_key_cannot_call_admin_sandbox_recheck() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Not Admin").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call("admin.sandbox_recheck", json!({}))
        .await
        .expect_err("a tenant key must never reach admin.sandbox_recheck");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
}
