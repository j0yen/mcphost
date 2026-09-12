//! AC1 (P0) — Given a tool and `host.trigger.set(kind="schedule", schedule=
//! "* * * * *")` on a pro tenant, When two firings pass, Then
//! `host.runs.list(trigger="schedule")` shows two runs with `trigger_ref`
//! equal to the trigger id and `done` results.
//!
//! The PRD's own AC1 text implies waiting a real 130s for two per-minute
//! cron boundaries to pass. This test proves the same underlying behavior
//! (each due firing enqueues a run, `trigger_ref` ties it back to the
//! trigger, the ledger accumulates one row per firing) by calling
//! `mcphost::triggers::tick_once` directly and forcing the trigger's own
//! `next_unix` due each time, rather than waiting on real wall-clock cron
//! alignment -- the same "property under test doesn't depend on the
//! sleep's length" compression `runs_ac01` already applies to its own
//! (shorter) real-time wait.

use crate::common;
use common::{TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn two_forced_firings_both_land_in_the_ledger() {
    let server = TestServer::start().await;
    let (ns, key, _tenant_id) = signup_and_make_pro(&server, "AC1 Tenant", "cus_ac1").await;
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
                json!({"tool": "pinger", "schedule": "* * * * *"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    for firing in 0..2 {
        // Force this firing due right now instead of waiting for the real
        // per-minute boundary `host.trigger.set` computed.
        let now = mcphost::state::now_unix();
        server
            .state
            .db
            .update_trigger_after_fire(trigger_id.clone(), Some(now), None, 0)
            .await
            .expect("force due");
        mcphost::triggers::tick_once(&server.state).await.expect("tick");

        // Wait for THIS firing to finish before forcing the next one due --
        // otherwise the still-`queued` first run makes requirement 3's own
        // overlap guard correctly skip the second (proving overlap
        // detection instead of AC1's back-to-back-firings claim).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let listed = extract_structured(
                &client
                    .tools_call("host.runs.list", json!({"trigger": "schedule"}))
                    .await
                    .expect("runs.list"),
            );
            let runs = listed["runs"].as_array().expect("runs array");
            let done_count = runs
                .iter()
                .filter(|r| r["status"] == json!("done") && r["trigger_ref"] == json!(trigger_id))
                .count();
            if done_count > firing {
                break;
            }
            if Instant::now() >= deadline {
                panic!("firing {firing} for {ns}'s trigger never reached done: {runs:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "schedule"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    let done: Vec<_> = runs
        .iter()
        .filter(|r| r["status"] == json!("done") && r["trigger_ref"] == json!(trigger_id))
        .collect();
    assert!(
        done.len() >= 2,
        "expected 2 done schedule runs for {ns}'s trigger, saw: {runs:?}"
    );
}
