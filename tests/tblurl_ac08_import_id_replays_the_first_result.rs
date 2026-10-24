//! PRD-mcphost-table-import-from-url
//! AC8 (P1) — Given two `host.table.append` calls with the same `import_id`
//! within 24 h, When the second runs, Then it returns the first result and the
//! row count does not change.

use crate::common;

use common::sypul::{Fixture, orders_csv};
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

async fn count(client: &McpClient, table: &str) -> i64 {
    call(client, "host.table.count", json!({"table": table})).await["count"].as_i64().expect("count")
}

#[tokio::test]
async fn tblurl_ac08_the_second_append_with_the_same_import_id_returns_the_first_result() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC8").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(40), false).await;
    call(&client, "host.table.create", json!({"name": "orders", "columns": {"id": "integer", "name": "text", "qty": "integer"}})).await;

    let args = json!({"table": "orders", "source_url": fixture.csv_url(), "import_id": "nightly-1"});
    let first = call(&client, "host.table.append", args.clone()).await;
    assert_eq!(first["rows_imported"], json!(40), "{first}");
    assert_eq!(count(&client, "orders").await, 40);
    assert_eq!(fixture.hits(), 1);

    let second = call(&client, "host.table.append", args).await;
    assert_eq!(second, first, "the second call returns the first result");
    assert_eq!(count(&client, "orders").await, 40, "the row count did not change");
    assert_eq!(fixture.hits(), 1, "a replay fetches nothing");

    // Another id imports again; so does the same id on csv_text for another table.
    let third = call(&client, "host.table.append", json!({"table": "orders", "source_url": fixture.csv_url(), "import_id": "nightly-2"})).await;
    assert_eq!(third["rows_imported"], json!(40), "{third}");
    assert_eq!(count(&client, "orders").await, 80);
    call(&client, "host.table.create", json!({"name": "other", "columns": {"id": "integer"}})).await;
    let other = call(&client, "host.table.append", json!({"table": "other", "csv_text": "id\n1\n", "import_id": "nightly-1"})).await;
    assert_eq!(other["rows_imported"], json!(1), "an import_id is scoped to its table: {other}");
}

#[tokio::test]
async fn tblurl_ac08_an_import_id_older_than_a_day_imports_again_and_a_failure_is_not_replayed() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC8 window").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    call(&client, "host.table.create", json!({"name": "t", "columns": {"id": "integer"}})).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");

    let args = json!({"table": "t", "csv_text": "id\n1\n2\n", "import_id": "k"});
    call(&client, "host.table.append", args.clone()).await;
    assert_eq!(count(&client, "t").await, 2);
    let db = rusqlite::Connection::open(server.data_dir.0.join("tables").join(format!("{}.db", tenant.id))).expect("tenant db");
    db.execute("UPDATE _mcphost_imports SET created_unix = created_unix - 90000", []).expect("age the ledger");
    let again = call(&client, "host.table.append", args).await;
    assert_eq!(again["rows_imported"], json!(2), "{again}");
    assert_eq!(count(&client, "t").await, 4, "past 24 h the id no longer replays");

    // A failed attempt under an id is not stored as its result: the retry runs.
    let bad = json!({"table": "t", "csv_text": "zzz\n1\n", "import_id": "retry"});
    client.tools_call("host.table.append", bad).await.expect_err("mismatch");
    let fixed = call(&client, "host.table.append", json!({"table": "t", "csv_text": "id\n9\n", "import_id": "retry"})).await;
    assert_eq!(fixed["rows_imported"], json!(1), "{fixed}");
}

#[tokio::test]
async fn tblurl_ac08_create_with_an_import_id_replays_too() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC8 create").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"name": "made", "csv_text": "id,name\n1,a\n2,b\n", "import_id": "make-once"});
    let first = call(&client, "host.table.create", args.clone()).await;
    let second = call(&client, "host.table.create", args).await;
    assert_eq!(second, first);
    assert_eq!(count(&client, "made").await, 2);
}
