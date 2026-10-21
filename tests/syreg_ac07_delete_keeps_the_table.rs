//! PRD-mcphost-sync-source-registry-and-create AC7 -- Given a sync, When
//! `host.sync.delete` runs, Then `host.sync.list` is empty, the table still
//! answers `host.table.query`, and a second `host.sync.create` with the same
//! `(kind, ref, table)` returns a new `sync_id`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

#[tokio::test]
async fn syreg_ac07_delete_keeps_the_table_and_recreate_mints_a_new_id() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYREG AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"source": {"kind": "csv_url", "ref": "https://data.example.com/orders.csv"}, "table": "orders"});

    let first = call(&client, "host.sync.create", args.clone()).await;
    // R7: the same (kind, ref, table) while it exists is the same sync.
    let again = call(&client, "host.sync.create", args.clone()).await;
    assert_eq!(again["sync_id"], first["sync_id"], "{again}");
    assert_eq!(again["existing"], true, "{again}");

    let deleted = call(&client, "host.sync.delete", json!({"sync_id": first["sync_id"]})).await;
    assert_eq!(deleted["deleted"], true, "{deleted}");
    let listed = call(&client, "host.sync.list", json!({})).await;
    assert_eq!(listed["syncs"].as_array().expect("syncs").len(), 0, "{listed}");

    let queried = call(&client, "host.table.query", json!({"sql": "SELECT * FROM orders"})).await;
    assert!(queried.get("rows").is_some(), "the table still answers host.table.query: {queried}");

    let second = call(&client, "host.sync.create", args).await;
    assert_ne!(second["sync_id"], first["sync_id"], "a new sync_id: {second}");
    assert_eq!(second["existing"], false, "{second}");

    let err = client
        .tools_call("host.sync.get", json!({"sync_id": first["sync_id"]}))
        .await
        .expect_err("the deleted sync is gone");
    assert_eq!(err.error_code.as_deref(), Some("not_found"), "{err:?}");
}
