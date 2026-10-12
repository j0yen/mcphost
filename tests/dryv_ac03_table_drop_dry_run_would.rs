//! PRD-mcphost-dry-run-on-write-verbs
//! AC3 -- Given `host.table.drop {name: "orders", dry_run: true}` on a
//! 4 812-row table, When called, Then the response is `{would: {tables:
//! [{name: "orders", action: "drop", rows: 4812, bytes: >0}]}, _meta:
//! {dry_run: true}}` and `host.table.query` afterwards returns 4 812 rows.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn dry_run_drop_reports_the_table_and_commits_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"sku": "text"}}))
        .await
        .expect("create orders");
    let rows: Vec<Value> = (0..4_812).map(|i| json!({"sku": format!("sku-{i}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append 4812 rows");

    let result = client
        .tools_call("host.table.drop", json!({"name": "orders", "dry_run": true}))
        .await
        .expect("dry-run drop ok");
    let structured = extract_structured(&result);

    let tables = structured["would"]["tables"].as_array().expect("would.tables").clone();
    assert_eq!(tables.len(), 1, "{structured}");
    assert_eq!(tables[0]["name"], json!("orders"), "{structured}");
    assert_eq!(tables[0]["action"], json!("drop"), "{structured}");
    assert_eq!(tables[0]["rows"], json!(4_812), "{structured}");
    assert!(tables[0]["bytes"].as_i64().unwrap_or(0) > 0, "{structured}");
    assert_eq!(structured["_meta"]["dry_run"], json!(true), "{structured}");
    assert_eq!(
        structured["would"].as_object().expect("would object").keys().collect::<Vec<_>>(),
        vec!["tables"],
        "absent classes are omitted: {structured}"
    );

    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM orders"}))
        .await
        .expect("table still queryable");
    assert_eq!(extract_structured(&queried)["rows"][0]["n"], json!(4_812));
}
