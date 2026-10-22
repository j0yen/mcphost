//! PRD-mcphost-sync-pull-with-change-detection AC3 -- Given a 100-row
//! snapshot with `primary_key: id`, When the fixture changes 2 rows, adds 1
//! and removes 1, Then the receipt reads `rows_updated: 2, rows_added: 1,
//! rows_deleted: 1`, `host.table.query` returns 100 rows with no duplicate
//! `id`, and the removed row is gone after the following run.

use std::collections::HashSet;

use crate::common::sypul::{Fixture, orders_csv};
use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

async fn ids(client: &McpClient) -> Vec<String> {
    let q = call(client, "host.table.query", json!({"sql": "SELECT id, name FROM orders ORDER BY id", "limit": 1000})).await;
    q["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("rows: {q}"))
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_else(|| panic!("id: {r}")).to_string())
        .collect()
}

#[tokio::test]
async fn sypul_ac03_two_updated_one_added_one_removed_leaves_100_rows_without_duplicates() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYPUL AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(100), true).await;
    let created = call(
        &client,
        "host.sync.create",
        json!({"source": {"kind": "csv_url", "ref": fixture.csv_url()}, "table": "orders", "primary_key": "id"}),
    )
    .await;
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();
    let now = mcphost::state::now_unix();

    mcphost::sync::run(&server.state, &sync_id, now).await.expect("first run");
    assert_eq!(ids(&client).await.len(), 100);

    // Rows 5 and 6 change, row 7 is removed, row 101 is new.
    let mut csv = String::from("id,name,qty\n");
    for i in (1..=101).filter(|i| *i != 7) {
        let name = if i == 5 || i == 6 { format!("renamed {i}") } else { format!("item {i}") };
        csv.push_str(&format!("{i},{name},{}\n", i % 7));
    }
    fixture.set_body(&csv);
    mcphost::sync::run(&server.state, &sync_id, now + 1).await.expect("second run");

    let got = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    let last = &got["last_run"];
    assert_eq!(last["status"], "ok", "{got}");
    assert_eq!(last["rows_updated"], 2, "{got}");
    assert_eq!(last["rows_added"], 1, "{got}");
    assert_eq!(last["rows_deleted"], 1, "{got}");
    assert_eq!(last["change_detected"], true, "{got}");

    let after = ids(&client).await;
    assert_eq!(after.len(), 100, "{after:?}");
    assert_eq!(after.iter().collect::<HashSet<_>>().len(), 100, "no duplicate id: {after:?}");
    assert!(!after.contains(&"7".to_string()), "the removed row is gone");
    assert!(after.contains(&"101".to_string()), "the added row is there");
    let renamed = call(&client, "host.table.query", json!({"sql": "SELECT name FROM orders WHERE id = '5'"})).await;
    assert_eq!(renamed["rows"][0]["name"], "renamed 5", "{renamed}");

    // The following run (the fixture unchanged) keeps all of it.
    mcphost::sync::run(&server.state, &sync_id, now + 2).await.expect("third run");
    let again = ids(&client).await;
    assert_eq!(again, after);
    let got = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    assert_eq!(got["last_run"]["change_detected"], false, "{got}");
    assert_eq!(got["last_snapshot_rows"], 100, "{got}");
}
