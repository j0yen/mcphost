//! PRD-mcphost-run-budget-governor
//! AC8 (P1) — Given a run crossing 0.8, When it continues, Then one
//! `run.budget_alert` event exists for it and `alerted_at_unix` is set
//! once.
//!
//! A ten-step chain under `budget: {max_child_calls: 11}` crosses the 0.8
//! alert fraction at its ninth child (9/11 ≈ 0.818) and keeps running to a
//! normal `done` completion (10/11 ≈ 0.909, never reaching `exceeded`) --
//! proving the alert fires mid-run, without needing the run to halt.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn one_budget_alert_event_and_alerted_at_unix_set_once() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    for i in 0..10 {
        publish(&client, &format!("step{i}"), "echo", json!({"schema": schema})).await;
    }
    let steps: Vec<serde_json::Value> =
        (0..10).map(|i| json!({"tool": format!("step{i}"), "args": {"n": i}})).collect();
    let chain = publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({
                    "name": "pipeline",
                    "args": {},
                    "async": true,
                    "budget": {"max_child_calls": 11},
                }),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(15);
    let done = loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            break got;
        }
        assert!(Instant::now() < deadline, "run never finalized within 15s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(done["status"], json!("done"), "{done}");
    let budget = &done["progress"]["budget"];
    assert_eq!(budget["verdict"], json!("alert"), "{budget}");
    let alerted_at_unix = budget["alerted_at_unix"].as_i64().expect("alerted_at_unix must be set");
    assert!(alerted_at_unix > 0, "{budget}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let alerts = extract_structured(
        &admin
            .tools_call("admin.alerts.list", json!({"limit": 100}))
            .await
            .expect("admin.alerts.list ok"),
    );
    let matching: Vec<&serde_json::Value> = alerts["alerts"]
        .as_array()
        .expect("alerts array")
        .iter()
        .filter(|a| a["key"] == json!(format!("run.budget_alert:{run_id}")))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one run.budget_alert event must exist for {run_id}: {alerts}"
    );
    assert_eq!(matching[0]["body"]["run_id"], json!(run_id), "{matching:?}");
    assert_eq!(matching[0]["repeat_count"], json!(0), "one raise, never collapsed: {matching:?}");
}
