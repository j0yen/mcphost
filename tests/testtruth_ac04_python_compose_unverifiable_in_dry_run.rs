//! PRD-mcphost-tool-test-truth AC4 (P0) — Given a python tool whose source
//! calls `mcphost.call(...)`, When `host.tool_test` runs, Then
//! `verdict: unverifiable`, `reason: "composition is not executed in a dry
//! run"`, `next.tool: "host_tool_call"`, and the error text is under
//! `detail`; and `host.tool_call` of the same tool succeeds.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn composing_tool_test_is_unverifiable_but_the_real_call_succeeds() {
    // See python_ac01's own comment: this test runs real sandboxed python
    // tools, which need unprivileged user namespaces -- not guaranteed on
    // GitHub's hosted runners.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let callee_spec = json!({
        "source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n",
        "args_schema": {"type": "object", "properties": {"n": {"type": "integer"}}},
        "network": "none",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "callee", "kind": "python", "spec": callee_spec}),
        )
        .await
        .expect("publish callee ok");

    let composer_spec = json!({
        "source": "import mcphost\n\ndef main(args):\n    return mcphost.call(\"callee\", {\"n\": 2})\n",
        "args_schema": {"type": "object"},
        "network": "none",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "composer", "kind": "python", "spec": composer_spec}),
        )
        .await
        .expect("publish composer ok");

    let test_result = poll_until_ready(
        &client,
        "host.tool_test",
        json!({"name": "composer", "args": {}}),
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_test must succeed, not error: {} {}", e.code, e.message));
    let structured = common::extract_structured(&test_result);

    assert_eq!(structured["verdict"], json!("unverifiable"), "{structured}");
    assert_eq!(
        structured["reason"],
        json!("composition is not executed in a dry run"),
        "{structured}"
    );
    assert_eq!(structured["next"]["tool"], json!("host_tool_call"), "{structured}");
    let detail = structured["detail"].as_str().expect("detail must be a string");
    assert!(
        detail.contains("McphostCallError"),
        "detail must carry the raw error text: {detail}"
    );

    let call_result = poll_until_ready(
        &client,
        "host.tool_call",
        json!({"name": "composer", "args": {}}),
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_call of the same tool must succeed: {} {}", e.code, e.message));
    let call_structured = common::extract_structured(&call_result);
    assert_eq!(
        call_structured["doubled"], 4,
        "the real call must still compose normally: {call_structured}"
    );
}
