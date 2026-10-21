//! PRD-mcphost-sync-source-registry-and-create AC6 (P0) -- Given an active
//! sync whose `next_run_unix` is in the past, When `tick_once` runs, Then one
//! `sync_runs` receipt exists for it, `next_run_unix` moved to the next cron
//! slot, and a second `tick_once` within the same slot writes no second
//! receipt.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::cron::CronSchedule;
use serde_json::json;

#[tokio::test]
async fn syreg_ac06() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Syreg AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let created = extract_structured(
        &client
            .tools_call(
                "host.sync.create",
                json!({"source": {"kind": "csv_url", "ref": "https://example.com/orders.csv"}, "table": "orders", "every": "*/10 * * * *"}),
            )
            .await
            .expect("create"),
    );
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();

    // Not due yet: a tick writes nothing.
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
    assert!(mcphost::sync::receipts_for_test(&server.state, &sync_id).await.unwrap().is_empty());

    // Make it due an hour ago.
    let now = mcphost::state::now_unix();
    mcphost::sync::set_next_run_for_test(&server.state, &sync_id, now - 3600).await.expect("backdate");
    mcphost::triggers::tick_once(&server.state).await.expect("tick");

    let receipts = mcphost::sync::receipts_for_test(&server.state, &sync_id).await.unwrap();
    assert_eq!(receipts, vec!["not_implemented".to_string()]);

    let slot = CronSchedule::parse("*/10 * * * *").unwrap();
    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    let next = listed["syncs"][0]["next_run_unix"].as_i64().expect("next_run_unix");
    assert!(next > now, "next_run_unix {next} moved past now {now}");
    assert!(next <= slot.next_after(mcphost::state::now_unix()).unwrap(), "next_run_unix {next} is the next cron slot");
    assert_eq!(next % 600, 0, "a */10 slot lands on a ten-minute boundary: {next}");

    // A second tick inside the same slot finds nothing due.
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
    assert_eq!(mcphost::sync::receipts_for_test(&server.state, &sync_id).await.unwrap().len(), 1);
}
