//! PRD-mcphost-error-envelope-schema-contract
//! AC4 (P0) -- Given a stored failure with `error_data_json` NULL, When
//! `host.runs.list` and `host.runs.get` render it, Then `error.data == {}` and
//! `error.kind` is preserved.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

async fn wait_finalized(client: &McpClient, run_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            return;
        }
        assert!(Instant::now() < deadline, "run {run_id} never finalized");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn errenv_ac04() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Errenv AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for i in 0..3 {
        publish(&client, &format!("step{i}"), "echo", json!({"schema": {"type": "object"}})).await;
    }
    let steps: Vec<Value> = (0..3).map(|i| json!({"tool": format!("step{i}"), "args": {}})).collect();
    publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    let enqueued = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "pipeline", "args": {}, "async": true, "budget": {"max_child_calls": 2}}),
            )
            .await
            .expect("enqueue"),
    );
    let run_id = enqueued["run_id"].as_str().expect("run_id").to_string();
    wait_finalized(&client, &run_id).await;

    // Make it a legacy row: the failure's data column was never written.
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open db");
    let touched = conn
        .execute("UPDATE runs SET error_data_json = NULL WHERE id = ?1", [&run_id])
        .expect("null the column");
    assert_eq!(touched, 1);
    drop(conn);

    let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));
    assert_eq!(got["error"]["data"], json!({}), "host.runs.get: {got}");
    assert_eq!(got["error"]["kind"], json!("budget_exceeded"), "host.runs.get: {got}");

    let listed = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("list"));
    let row = listed["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .find(|r| r["run_id"] == json!(run_id))
        .unwrap_or_else(|| panic!("run missing from list: {listed}"));
    assert_eq!(row["error"]["data"], json!({}), "host.runs.list: {row}");
    assert_eq!(row["error"]["kind"], json!("budget_exceeded"), "host.runs.list: {row}");
}
