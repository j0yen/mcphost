//! PRD-mcphost-sync-pull-with-change-detection AC2 -- Given a `csv_url` sync
//! whose fixture answers 304 on the second fetch, When two runs complete,
//! Then the second receipt has `change_detected: false`, `rows_added: 0`,
//! `provider_calls: 1`, and the table row count is unchanged.

use crate::common::sypul::{Fixture, orders_csv};
use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

#[tokio::test]
async fn sypul_ac02_second_fetch_answers_304_and_the_receipt_says_nothing_changed() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYPUL AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(12), true).await;

    let created = call(
        &client,
        "host.sync.create",
        json!({"source": {"kind": "csv_url", "ref": fixture.csv_url()}, "table": "orders", "primary_key": "id"}),
    )
    .await;
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();
    let now = mcphost::state::now_unix();

    mcphost::sync::run(&server.state, &sync_id, now).await.expect("first run");
    let first = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    assert_eq!(first["last_run"]["status"], "ok", "{first}");
    assert_eq!(first["last_run"]["change_detected"], true, "{first}");
    assert_eq!(first["last_run"]["rows_added"], 12, "{first}");
    assert_eq!(first["last_run"]["provider_calls"], 1, "{first}");
    let before = call(&client, "host.table.count", json!({"table": "orders"})).await;
    assert_eq!(before["count"], 12, "{before}");

    mcphost::sync::run(&server.state, &sync_id, now + 1).await.expect("second run");
    let second = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    let last = &second["last_run"];
    assert_eq!(last["status"], "ok", "{second}");
    assert_eq!(last["change_detected"], false, "{second}");
    assert_eq!(last["rows_added"], 0, "{second}");
    assert_eq!(last["rows_updated"], 0, "{second}");
    assert_eq!(last["rows_deleted"], 0, "{second}");
    assert_eq!(last["provider_calls"], 1, "{second}");
    assert_eq!(last["bytes"], 0, "a 304 carries no body: {second}");
    assert_ne!(last["run_id"], first["last_run"]["run_id"], "{second}");
    assert_eq!(second["runs"].as_array().expect("runs").len(), 2, "newest first: {second}");
    assert_eq!(second["runs"][0]["run_id"], last["run_id"], "{second}");
    assert_eq!(second["rows_in_table"], 12, "{second}");
    assert_eq!(second["drift"], false, "{second}");

    let after = call(&client, "host.table.count", json!({"table": "orders"})).await;
    assert_eq!(after["count"], before["count"], "{after}");
    assert_eq!(fixture.hits(), 2, "one request per run");
}
