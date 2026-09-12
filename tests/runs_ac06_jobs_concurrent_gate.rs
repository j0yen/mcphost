//! AC6 (P0) — Given a free tenant with `jobs_concurrent: 1`, When two jobs
//! are submitted, Then the second stays `queued` until the first finishes
//! and `host.runs.list` shows both.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn second_job_stays_queued_until_the_first_finishes() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(3)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 30,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gate_me", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.gate_me"),
        json!({}),
        Duration::from_secs(15),
    )
    .await;

    let first = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "gate_me", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue first ok"),
    );
    let second = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "gate_me", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue second ok"),
    );
    let first_id = first["run_id"].as_str().expect("run_id").to_string();
    let second_id = second["run_id"].as_str().expect("run_id").to_string();
    assert_ne!(first_id, second_id);

    // Wait for the first to actually start running (free plan's
    // `jobs_concurrent: 1` should then hold the second `queued`).
    let running_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": first_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("running") {
            break;
        }
        assert!(Instant::now() < running_deadline, "first job never started: {got}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let second_status = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": second_id}))
            .await
            .expect("runs.get ok"),
    );
    assert_eq!(
        second_status["status"],
        json!("queued"),
        "second job must stay queued while the first (jobs_concurrent: 1) is running: {second_status}"
    );

    let list = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"tool": "gate_me"}))
            .await
            .expect("runs.list ok"),
    );
    let ids: Vec<String> = list["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .map(|r| r["run_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(ids.contains(&first_id), "list must show the first run: {list}");
    assert!(ids.contains(&second_id), "list must show the second run: {list}");

    // Both eventually finish (second job leases once the first's slot frees).
    let both_done_deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let a = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": first_id}))
                .await
                .expect("runs.get ok"),
        );
        let b = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": second_id}))
                .await
                .expect("runs.get ok"),
        );
        if a["status"] == json!("done") && b["status"] == json!("done") {
            return;
        }
        assert!(
            Instant::now() < both_done_deadline,
            "both jobs must eventually finish: {a} / {b}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
