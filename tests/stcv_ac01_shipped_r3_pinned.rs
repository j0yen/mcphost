//! PRD-mcphost-sandbox-timeout-clamp-visible
//! AC1 (P0) -- Given a free tenant, When it publishes a python spec with
//! `memory_mb: 512`, Then the publish is refused with `plan_required` naming
//! `sandbox_memory_mb_max` / 256 / `memory_mb` / 512; and a spec with
//! `memory_mb: 256`, `timeout_s: 45` is accepted with `version` 1 and its call
//! runs under cpu 30 s / wall 32 s (shipped R3, pinned).
//!
//! cpu is read from inside the sandbox (`RLIMIT_CPU`); wall is shown by a
//! sleeping tool being killed before it can finish.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const SRC: &str = "import resource, time\ndef main(args):\n    if args.get(\"sleep\"):\n        time.sleep(args[\"sleep\"])\n    return {\"cpu\": resource.getrlimit(resource.RLIMIT_CPU)[0]}\n";

#[tokio::test]
async fn stcv_ac01_memory_refused_timeout_accepted_and_clamped_at_call() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "STCV AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "big", "kind": "python", "spec": {"source": SRC, "args_schema": {"type": "object"}, "memory_mb": 512}}),
        )
        .await
        .expect_err("free tenant publishing memory_mb: 512 must be refused");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"));
    assert_eq!(err.data["field"], json!("sandbox_memory_mb_max"));
    assert_eq!(err.data["limit"], json!(256));
    assert_eq!(err.data["spec_field"], json!("memory_mb"));
    assert_eq!(err.data["requested"], json!(512));

    let published = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "fits", "kind": "python", "spec": {"source": SRC, "args_schema": {"type": "object"}, "memory_mb": 256, "timeout_s": 45}}),
        )
        .await
        .expect("memory_mb 256 + timeout_s 45 is accepted on free");
    assert_eq!(extract_structured(&published)["version"], json!(1));

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow", "kind": "python", "spec": {"source": SRC, "args_schema": {"type": "object"}, "memory_mb": 256, "timeout_s": 45}}),
        )
        .await
        .expect("publish slow");

    // Both calls below are each tool's FIRST (cold) call: a warm hit is sized
    // by the plan backstop, not the per-call budget.
    let wait_built = |name: String, args: serde_json::Value| {
        let client = &client;
        async move {
            for _ in 0..600 {
                match client.tools_call(&name, args.clone()).await {
                    Err(e) if e.error_code.as_deref() == Some("tool_building") => {}
                    other => return other,
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("tool never became ready");
        }
    };
    let name = format!("{ns}.fits");
    let first = wait_built(name, json!({})).await.expect("first call");
    let out = extract_structured(&first);
    assert_eq!(out["cpu"], json!(30), "cpu limit is min(45, 30): {out}");

    // The 40 s sleep outlives the 32 s wall; were the spec's 45 s honoured
    // it would finish, so a kill proves the plan's wall applied.
    let slow = wait_built(format!("{ns}.slow"), json!({"sleep": 40})).await;
    assert!(slow.is_err(), "a 40 s sleep must be killed at the wall: {slow:?}");
}
