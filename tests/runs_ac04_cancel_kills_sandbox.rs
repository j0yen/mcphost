//! AC4 (P0) — Given a running job, When `host.runs.cancel` is called, Then
//! the sandbox process is gone within 2 s and the run reads `cancelled`.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn cancel_kills_the_running_sandbox() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(30)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 45,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "cancel_me", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.cancel_me"),
        json!({}),
        Duration::from_secs(15),
    )
    .await;

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "cancel_me", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    // Wait for the job to actually be running (a live sandbox pid) before
    // cancelling it -- cancelling a still-queued job is a different case
    // (nothing to kill), covered implicitly by `cancel`'s own `killed:
    // false` branch, not this AC.
    let running_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("running") {
            break;
        }
        assert!(
            Instant::now() < running_deadline,
            "job never reached running within 15s: {got}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let cancel_started = Instant::now();
    let cancel = extract_structured(
        &client
            .tools_call("host.runs.cancel", json!({"run_id": run_id}))
            .await
            .expect("cancel ok"),
    );
    assert_eq!(cancel["cancelled"], json!(true), "cancel: {cancel}");
    assert_eq!(cancel["killed"], json!(true), "a running job must have had a live pid: {cancel}");

    let cancel_deadline = cancel_started + Duration::from_secs(2);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("cancelled") {
            return;
        }
        if Instant::now() >= cancel_deadline {
            panic!("run must read cancelled within 2s of host.runs.cancel: {got}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
