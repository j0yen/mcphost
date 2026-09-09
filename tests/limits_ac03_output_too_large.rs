//! AC3 (PRD-mcphost-call-limits-honest) — Given a python tool returning a
//! 3 MB string, When called, Then the error is `tool_output_too_large` with
//! `limit_bytes` 1048576 and `actual_bytes` ≥ 3,000,000, and the same tool
//! returning 100 KB succeeds.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn oversized_output_is_reported_as_tool_output_too_large() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"blob\": \"x\" * args.get(\"n\", 0)}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bigout", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.bigout");

    // 100 KB succeeds.
    let ok = poll_until_ready(
        &client,
        &qualified,
        json!({"n": 100 * 1024}),
        Duration::from_secs(20),
    )
    .await
    .expect("a 100KB result must succeed");
    let structured = common::extract_structured(&ok);
    assert_eq!(
        structured["blob"]
            .as_str()
            .map(str::len)
            .unwrap_or_default(),
        100 * 1024
    );

    // 3 MB fails, naming the cap and the size produced.
    let err = client
        .tools_call(&qualified, json!({"n": 3_000_000}))
        .await
        .expect_err("a 3MB result must be refused as too large");
    assert_eq!(err.error_code.as_deref(), Some("tool_output_too_large"));
    assert_eq!(
        err.data.get("limit_bytes").and_then(serde_json::Value::as_u64),
        Some(1_048_576)
    );
    let actual_bytes = err
        .data
        .get("actual_bytes")
        .and_then(serde_json::Value::as_u64)
        .expect("actual_bytes must be present");
    assert!(
        actual_bytes >= 3_000_000,
        "actual_bytes must reflect the true (>=3MB) size, got {actual_bytes}"
    );
}
