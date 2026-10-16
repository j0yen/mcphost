//! PRD-mcphost-csv-to-table-one-call
//! AC3 -- Given a CSV with `"n/a"` at line 412 of column `amount`, When
//! loaded with defaults, Then `rows_loaded` is total minus 1 and
//! `skipped[0]` is `{row: 412, column: "amount", reason: "not a real: 'n/a'"}`;
//! with `on_error: abort` no table exists afterwards.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const TOTAL: usize = 600;

fn csv_with_bad_cell() -> String {
    let mut s = String::from("id,amount\n");
    for id in 1..=TOTAL {
        // line = id + 1, so id 411 sits on file line 412.
        let amount = if id == 411 { "n/a".to_string() } else { format!("{id}.25") };
        s.push_str(&format!("{id},{amount}\n"));
    }
    s
}

#[tokio::test]
async fn csvtab_ac03() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CSVTAB AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.docs.put", json!({"name": "sales.csv", "content": csv_with_bad_cell()}))
        .await
        .expect("put csv");

    let loaded = extract_structured(
        &client
            .tools_call("host.table.from_blob", json!({"doc": "sales.csv", "name": "sales"}))
            .await
            .expect("default load succeeds"),
    );
    assert_eq!(loaded["rows_loaded"], json!(TOTAL - 1), "{loaded}");
    assert_eq!(loaded["rows_skipped"], json!(1), "{loaded}");
    assert_eq!(
        loaded["skipped"][0],
        json!({"row": 412, "column": "amount", "reason": "not a real: 'n/a'"}),
        "{loaded}"
    );
    let amount = loaded["columns"].as_array().unwrap().iter().find(|c| c["name"] == "amount").unwrap();
    assert_eq!(amount["type"], json!("real"), "one stray cell must not demote the column: {loaded}");
    let counted = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM sales"}))
            .await
            .expect("query"),
    );
    assert_eq!(counted["rows"][0]["n"], json!(TOTAL - 1), "{counted}");

    let aborted = client
        .tools_call(
            "host.table.from_blob",
            json!({"doc": "sales.csv", "name": "sales_strict", "on_error": "abort"}),
        )
        .await;
    let err = aborted.expect_err("abort must refuse the whole load");
    assert!(err.message.contains("row 412") && err.message.contains("amount"), "{}", err.message);
    let listed = extract_structured(&client.tools_call("host.table.list", json!({})).await.expect("list"));
    let names: Vec<&str> = listed["tables"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["sales"], "no sales_strict table may exist after an aborted load: {listed}");
}
