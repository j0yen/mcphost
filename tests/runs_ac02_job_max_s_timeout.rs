//! AC2 (P0) — Given the same tool with a plan `job_max_s` of 60, When run
//! as a job, Then the run ends `timeout` at about 60 s and the last
//! progress is kept.
//!
//! The real free-plan `job_max_s` is 300s (too long to wait out in a
//! test), so this inserts the `queued` run directly with a short
//! `deadline_s` (`Db::insert_queued_run` takes it as a raw argument, not
//! derived from the plan) rather than through `host.tool_call`'s own
//! enqueue path -- the real background executor (started by
//! `common::TestServer` exactly as production does) still leases and runs
//! it normally; only the deadline is shortened.

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn job_times_out_at_its_deadline_keeping_last_progress() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time, mcphost\n\
def main(args):\n    \
    mcphost.progress(50, \"half\")\n    \
    time.sleep(30)\n    \
    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 45,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "too_slow", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.too_slow"),
        json!({}),
        Duration::from_secs(15),
    )
    .await;

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let run_id = mcphost::state::new_ulid();
    server
        .state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            "too_slow".to_string(),
            "job".to_string(),
            None,
            None,
            3, // deadline_s: well under the tool's own 30s sleep
            "{}".to_string(),
        )
        .await
        .expect("insert_queued_run");

    let started = Instant::now();
    let deadline = started + Duration::from_secs(20);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("timeout") {
            assert_eq!(
                got["progress"],
                json!({"pct": 50, "msg": "half"}),
                "last progress must be kept on timeout: {got}"
            );
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_secs(15),
                "a 3s deadline must time out well under 15s of test slack, took {elapsed:?}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "job never timed out within 20s: {got}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
