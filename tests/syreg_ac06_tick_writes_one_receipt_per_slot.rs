//! PRD-mcphost-sync-source-registry-and-create AC6 -- Given an active sync
//! whose `next_run_unix` is in the past, When `tick_once` runs, Then one
//! `sync_runs` receipt exists for it, `next_run_unix` moved to the next cron
//! slot, and a second `tick_once` within the same slot writes no second
//! receipt.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

#[tokio::test]
async fn syreg_ac06_due_sync_gets_one_receipt_and_moves_to_the_next_slot() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYREG AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let created = call(
        &client,
        "host.sync.create",
        // Nothing listens on port 9: the pull ends in an `error` receipt at once.
        json!({"source": {"kind": "csv_url", "ref": "http://127.0.0.1:9/orders.csv"}, "table": "orders"}),
    )
    .await;
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();

    // Not due yet: a tick writes nothing.
    mcphost::triggers::tick_once(&server.state).await.expect("tick before due");
    assert_eq!(mcphost::sync::receipt_count_for_test(&server.state, &sync_id).await.expect("count"), 0);

    let now = mcphost::state::now_unix();
    mcphost::sync::set_next_run_for_test(&server.state, &sync_id, now - 120).await.expect("move into the past");
    mcphost::triggers::tick_once(&server.state).await.expect("first tick");
    assert_eq!(mcphost::sync::receipt_count_for_test(&server.state, &sync_id).await.expect("count"), 1);

    let got = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    assert_eq!(got["last_run"]["status"], "error", "{got}");
    let next = got["next_run_unix"].as_i64().expect("next_run_unix");
    assert!(next > now && next <= now + 15 * 60 + 60, "next_run_unix {next} is the next */15 slot after {now}: {got}");
    assert_eq!(next % (15 * 60), 0, "a */15 slot sits on a quarter hour: {got}");

    // Same slot: nothing is due, so no second receipt.
    mcphost::triggers::tick_once(&server.state).await.expect("second tick");
    assert_eq!(mcphost::sync::receipt_count_for_test(&server.state, &sync_id).await.expect("count"), 1);

    // `host.sync.run_now` makes the sync due again; this call also feeds the
    // outsch harness capture the verb's real `structuredContent`.
    let ran = call(&client, "host.sync.run_now", json!({"sync_id": sync_id})).await;
    assert_eq!(ran["sync_id"], json!(sync_id), "{ran}");
    assert!(ran["next_run_unix"].as_i64().is_some_and(|n| n >= now), "run_now stamps next_run_unix: {ran}");
}
