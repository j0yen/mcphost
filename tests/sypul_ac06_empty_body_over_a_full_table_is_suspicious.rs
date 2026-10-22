//! PRD-mcphost-sync-pull-with-change-detection AC6 -- Given a non-empty table
//! and a fixture that returns an empty body, When the run completes, Then
//! `status: suspicious_empty`, `rows_deleted: 0`, and the table is intact.

use crate::common::sypul::{Fixture, orders_csv};
use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

#[tokio::test]
async fn sypul_ac06_an_empty_body_never_wipes_a_full_table() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYPUL AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(8), true).await;
    let created = call(
        &client,
        "host.sync.create",
        json!({"source": {"kind": "csv_url", "ref": fixture.csv_url()}, "table": "orders", "primary_key": "id"}),
    )
    .await;
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();
    let now = mcphost::state::now_unix();
    mcphost::sync::run(&server.state, &sync_id, now).await.expect("first run");
    let full = call(&client, "host.table.query", json!({"sql": "SELECT id, name, qty FROM orders ORDER BY id"})).await;
    assert_eq!(full["rows"].as_array().expect("rows").len(), 8, "{full}");

    // An empty body, then a header-only body: both are suspicious.
    for (i, body) in ["", "id,name,qty\n"].into_iter().enumerate() {
        fixture.set_body(body);
        mcphost::sync::run(&server.state, &sync_id, now + 1 + i as i64).await.expect("empty run");
        let got = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
        assert_eq!(got["last_run"]["status"], "suspicious_empty", "{got}");
        assert_eq!(got["last_run"]["rows_deleted"], 0, "{got}");
        assert_eq!(got["last_run"]["change_detected"], false, "{got}");
        assert_eq!(got["status"], "active", "the sync keeps running: {got}");
        let intact = call(&client, "host.table.query", json!({"sql": "SELECT id, name, qty FROM orders ORDER BY id"})).await;
        assert_eq!(intact["rows"], full["rows"], "the table is intact");
    }

    // The source comes back: the next run is an ordinary ok run, rows unchanged.
    fixture.set_body(&orders_csv(8));
    mcphost::sync::run(&server.state, &sync_id, now + 3).await.expect("recovery run");
    let got = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    assert_eq!(got["last_run"]["status"], "ok", "{got}");
    assert_eq!(got["last_run"]["rows_updated"], 0, "{got}");
    assert_eq!(got["last_run"]["rows_added"], 0, "{got}");
}
