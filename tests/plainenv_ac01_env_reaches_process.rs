//! PRD-mcphost-python-kind-plain-env AC1 (P0) — Given a python publish with
//! `env: {"UPSTREAM_URL": "https://example.test"}`, When the tool is
//! called, Then the process observes `UPSTREAM_URL` with that value and the
//! call succeeds.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn published_env_value_is_observed_by_the_process() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces -- same
    // skip-clean-in-CI, fail-loud-elsewhere pattern as every other
    // sandbox-dependent test in this suite (see tests/python_ac11_secret_
    // redaction.rs's own comment on this).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\ndef main(args):\n    return {\"url\": os.environ.get('UPSTREAM_URL')}\n",
        "args_schema": {"type": "object"},
        "env": {"UPSTREAM_URL": "https://example.test"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "env_tool", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.env_tool"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["url"], json!("https://example.test"));
}
