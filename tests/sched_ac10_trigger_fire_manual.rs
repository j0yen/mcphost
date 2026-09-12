//! AC10 (P1) — Given `host.trigger.fire(id)`, When called, Then one run
//! starts within 2s marked `manual: true`.

use crate::common;
use common::{TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn trigger_fire_enqueues_a_manual_run_immediately() {
    let server = TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "AC10 Tenant", "cus_ac10").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                // Well beyond pro's 60s minimum, so `next_unix` is far in
                // the future -- proving `fire` runs independent of it.
                json!({"tool": "pinger", "schedule": "0 0 1 1 *"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let started = Instant::now();
    let fired = extract_structured(
        &client
            .tools_call("host.trigger.fire", json!({"id": trigger_id}))
            .await
            .expect("trigger.fire"),
    );
    assert_eq!(fired["manual"], json!(true), "{fired:?}");
    let run_id = fired["run_id"].as_str().expect("run_id").to_string();

    let deadline = started + Duration::from_secs(2);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        assert_eq!(got["manual"], json!(true), "{got:?}");
        if got["status"] != json!("queued") {
            return; // started (running or already done) within the bound.
        }
        if Instant::now() >= deadline {
            panic!("manual fire never left queued within 2s: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
