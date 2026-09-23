//! PRD-mcphost-python-dependency-policy
//! AC9 (P2) — Given `admin.usage`, When read, Then tools are counted by
//! network mode and advisory state.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn admin_usage_tallies_tools_by_network_mode_and_advisory_state() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // SAFETY: this file has exactly one #[tokio::test] fn.
    unsafe {
        std::env::set_var("MCPHOST_ADVISORY_MODE", "warn");
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns_a, key_a) = signup(&server.base_url, "AC9 Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    // A clean, `network: none` (default) tool.
    client_a
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "clean",
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    return {\"ok\": True}\n",
                    "args_schema": {"type": "object"},
                },
            }),
        )
        .await
        .expect("publish clean");

    // A flagged tool -- `warn` mode lets a known-advisory publish through,
    // recorded on its lock from the start (AC3's own embedded seed).
    client_a
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "flagged",
                "kind": "python",
                "spec": {
                    "source": "import urllib3\ndef main(args):\n    return {\"ok\": True}\n",
                    "requirements": ["urllib3==1.26.4"],
                    "args_schema": {"type": "object"},
                },
            }),
        )
        .await
        .expect("publish flagged (warn mode must not fail it)");

    unsafe {
        std::env::remove_var("MCPHOST_ADVISORY_MODE");
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let usage = extract_structured(
        &admin
            .tools_call("admin.usage", json!({"window": "24h"}))
            .await
            .expect("admin.usage"),
    );

    let none_count = usage["tools_by_network"]["none"].as_i64().unwrap_or(0);
    assert!(
        none_count >= 2,
        "both published tools default to network:none: {usage:?}"
    );

    let clean = usage["tools_by_advisory_state"]["clean"].as_i64().unwrap_or(0);
    let advisory = usage["tools_by_advisory_state"]["advisory"].as_i64().unwrap_or(0);
    assert!(clean >= 1, "{usage:?}");
    assert!(advisory >= 1, "{usage:?}");
}
