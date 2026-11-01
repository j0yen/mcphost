//! PRD-mcphost-table-import-from-url
//! AC1 (P0) — Given a `host.table.create` call with `source_url` to a served
//! 1,000-row CSV with header, When the call completes, Then the table exists
//! with 1,000 rows, inferred column types, and the result reports
//! `rows_imported 1000, rows_skipped 0, source url`.

use crate::common;

use common::sypul::{Fixture, orders_csv};
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

#[tokio::test]
async fn tblurl_ac01_a_served_1000_row_csv_becomes_a_typed_table() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(1000), false).await;

    let created = call(&client, "host.table.create", json!({"name": "orders", "source_url": fixture.csv_url()})).await;
    assert_eq!(created["created"], json!(true), "{created}");
    assert_eq!(created["rows_imported"], json!(1000), "{created}");
    assert_eq!(created["rows_skipped"], json!(0), "{created}");
    assert_eq!(created["source"], json!("url"), "{created}");
    assert!(created["source_host"].as_str().is_some_and(|h| !h.is_empty()), "{created}");
    let columns: Vec<(String, String)> = created["columns"]
        .as_array()
        .expect("columns[]")
        .iter()
        .map(|c| (c["name"].as_str().unwrap().to_string(), c["type"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        columns,
        vec![("id".into(), "integer".into()), ("name".into(), "text".into()), ("qty".into(), "integer".into())],
        "inferred types"
    );

    let count = call(&client, "host.table.count", json!({"table": "orders"})).await;
    assert_eq!(count["count"], json!(1000), "{count}");
    let schema = call(&client, "host.table.schema", json!({"table": "orders"})).await;
    assert_eq!(schema["columns"]["id"], json!("integer"), "{schema}");
    assert_eq!(schema["columns"]["name"], json!("text"), "{schema}");
    let sum = call(&client, "host.table.query", json!({"sql": "SELECT SUM(qty) AS s, MAX(id) AS m FROM orders"})).await;
    assert_eq!(sum["rows"][0]["m"], json!(1000), "{sum}");
    assert_eq!(fixture.hits(), 1, "one fetch");
}

#[tokio::test]
async fn tblurl_ac01_mixed_columns_infer_bool_real_and_text() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC1 types").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start("Order ID,paid,price,note\n1,true,1.5,\"a, b\"\n2,false,2,\"say \"\"hi\"\"\"\n3,true,3.25,plain\n", false).await;

    let created = call(&client, "host.table.create", json!({"name": "mixed", "source_url": fixture.csv_url()})).await;
    assert_eq!(created["rows_imported"], json!(3), "{created}");
    let schema = call(&client, "host.table.schema", json!({"table": "mixed"})).await;
    assert_eq!(schema["columns"]["Order_ID"], json!("integer"), "{schema}");
    assert_eq!(schema["columns"]["paid"], json!("boolean"), "{schema}");
    assert_eq!(schema["columns"]["price"], json!("real"), "{schema}");
    assert_eq!(schema["columns"]["note"], json!("text"), "{schema}");
    let q = call(&client, "host.table.query", json!({"sql": "SELECT note FROM mixed WHERE Order_ID = 2"})).await;
    assert_eq!(q["rows"][0]["note"], json!("say \"hi\""), "{q}");
}
