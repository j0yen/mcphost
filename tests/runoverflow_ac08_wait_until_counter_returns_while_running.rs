//! PRD-mcphost-run-result-overflow-to-state
//! AC8 (P1) — Given `host.runs.wait {run_id, until: {counter:
//! "items_processed", gte: 1000}}`, When the tool reports 1000 at 3 s into
//! a 20 s run, Then wait returns at ~3 s with `status: "running"` and the
//! counters.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn wait_until_counter_returns_early_while_the_run_is_still_running() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "RunOverflow AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(20)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "longjob", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.longjob");
    let _ = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(15))
        .await
        .expect("warm-up call ok");

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "longjob", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    // Simulate "the tool reports 1000 at 3s into a 20s run" -- host.progress
    // is callable by id from outside the running tool's own process too.
    let reporter_run_id = run_id.clone();
    let reporter_client = common::McpClient::with_bearer(&server.base_url, &key);
    let reporter = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        reporter_client
            .tools_call(
                "host.progress",
                json!({"run_id": reporter_run_id, "counters": {"items_processed": 1000}}),
            )
            .await
            .expect("progress report ok");
    });

    let started = Instant::now();
    let waited = extract_structured(
        &client
            .tools_call(
                "host.runs.wait",
                json!({
                    "run_id": run_id,
                    "timeout_s": 25,
                    "until": {"counter": "items_processed", "gte": 1000},
                }),
            )
            .await
            .expect("wait ok"),
    );
    let elapsed = started.elapsed();
    reporter.await.expect("reporter task ok");

    assert!(
        elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(10),
        "wait must return around the 3s report, not the full 20s run or the 25s timeout: {elapsed:?}"
    );
    assert_eq!(waited["status"], json!("running"), "run: {waited}");
    assert_eq!(waited["counters"]["items_processed"], json!(1000), "run: {waited}");
}
