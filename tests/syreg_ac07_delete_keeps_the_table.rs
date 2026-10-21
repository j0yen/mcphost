//! PRD-mcphost-sync-source-registry-and-create AC7 (P1) -- Given a sync, When
//! `host.sync.delete` runs, Then `host.sync.list` is empty, the table still
//! answers `host.table.query`, and a second `host.sync.create` with the same
//! `(kind, ref, table)` returns a new `sync_id`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

fn args() -> Value {
    json!({"source": {"kind": "csv_url", "ref": "https://example.com/orders.csv"}, "table": "orders"})
}

#[tokio::test]
async fn syreg_ac07() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Syreg AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let first = extract_structured(&client.tools_call("host.sync.create", args()).await.expect("create"));
    let first_id = first["sync_id"].as_str().expect("sync_id").to_string();

    // While it lives, the same (kind, ref, table) is the same sync.
    let again = extract_structured(&client.tools_call("host.sync.create", args()).await.expect("repeat create"));
    assert_eq!(again["sync_id"], json!(first_id));
    assert_eq!(again["existing"], json!(true));

    let got = extract_structured(&client.tools_call("host.sync.get", json!({"sync_id": first_id})).await.expect("get"));
    assert_eq!(got["sync_id"], json!(first_id));
    assert_eq!(got["last_run"], Value::Null);

    let deleted = extract_structured(&client.tools_call("host.sync.delete", json!({"sync_id": first_id})).await.expect("delete"));
    assert_eq!(deleted["deleted"], json!(true));

    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    assert_eq!(listed["syncs"], json!([]), "{listed}");

    // The table is still there and answers a query.
    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM orders"}))
        .await
        .expect("the table survives its sync");
    assert_eq!(extract_structured(&queried)["rows"][0]["n"], json!(0));

    // Deleting again is a clear refusal, not a silent success.
    let gone = client
        .tools_call("host.sync.delete", json!({"sync_id": first_id}))
        .await
        .expect_err("the sync is gone");
    assert_eq!(gone.error_code.as_deref(), Some("sync_not_found"), "{gone:?}");

    // The same (kind, ref, table) now creates a new sync.
    let second = extract_structured(&client.tools_call("host.sync.create", args()).await.expect("second create"));
    assert_ne!(second["sync_id"], json!(first_id));
    assert!(second.get("existing").is_none(), "{second}");

    // run_now makes it due and the next tick writes its receipt.
    let second_id = second["sync_id"].as_str().unwrap().to_string();
    client.tools_call("host.sync.run_now", json!({"sync_id": second_id})).await.expect("run_now");
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
    let got = extract_structured(&client.tools_call("host.sync.get", json!({"sync_id": second_id})).await.expect("get"));
    assert_eq!(got["last_run"]["status"], "not_implemented", "{got}");
}
