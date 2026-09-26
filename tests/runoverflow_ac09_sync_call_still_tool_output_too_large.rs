//! PRD-mcphost-run-result-overflow-to-state
//! AC9 (P0) — Given a synchronous `host.tool_call` of the same 1.4 MiB
//! python tool used by the async ACs in this PRD, When called without
//! `async: true`, Then it still returns `tool_output_too_large` exactly as
//! before this PRD -- the sync path's output cap is untouched by the
//! async result-overflow-to-parts mechanism.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn sync_call_of_the_oversized_tool_is_still_tool_output_too_large() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "RunOverflow AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"blob\": \"x\" * args.get(\"n\", 0)}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bigjob", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.bigjob");
    let _ = poll_until_ready(&client, &qualified, json!({"n": 1}), Duration::from_secs(15))
        .await
        .expect("warm-up call ok");

    // Same 1.4 MiB size used by AC1/AC6's async result, but called
    // synchronously (no `async: true`): the 1 MiB sync output cap still
    // applies, unaffected by the async parts mechanism.
    let err = client
        .tools_call(
            "host.tool_call",
            json!({"name": "bigjob", "args": {"n": 1_468_006}}),
        )
        .await
        .expect_err("a synchronous 1.4MiB result must still be refused as too large");
    assert_eq!(err.error_code.as_deref(), Some("tool_output_too_large"));
    assert_eq!(
        err.data.get("limit_bytes").and_then(serde_json::Value::as_u64),
        Some(1_048_576)
    );
}
