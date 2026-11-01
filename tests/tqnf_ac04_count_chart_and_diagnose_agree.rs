//! PRD-mcphost-table-query-names-missing-table
//! AC4 -- Given `host.table.count`, `host.table.chart` and
//! `host.table.query_diagnose` with `staging_orders`, When each runs, Then
//! count and chart return the AC 1 error, and `query_diagnose` returns a
//! success result whose `data` block equals AC 1's `data`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

/// The keys the diagnosis contributes to `data` (the rest of the wire `data`
/// -- `request_id`, `class`, `help_url`, ... -- is added per reply).
const DIAGNOSIS_KEYS: [&str; 5] = ["error_code", "table", "candidates", "hint", "declared_tables_count"];

fn diagnosis_block(data: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in DIAGNOSIS_KEYS {
        out.insert(k.to_string(), data[k].clone());
    }
    Value::Object(out)
}

#[tokio::test]
async fn tqnf_ac04() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tqnf AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for (name, columns) in [
        ("orders", json!({"id": "integer", "total": "real", "status": "text"})),
        ("customers", json!({"id": "integer", "email": "text"})),
    ] {
        client
            .tools_call("host.table.create", json!({"name": name, "columns": columns}))
            .await
            .expect("create");
    }

    let sql = "SELECT * FROM staging_orders";
    let query_err = client
        .tools_call("host.table.query", json!({"sql": sql}))
        .await
        .expect_err("query: staging_orders is not declared");
    assert_eq!(query_err.error_code.as_deref(), Some("table_not_found"), "{query_err:?}");
    let want = diagnosis_block(&query_err.data);
    assert_eq!(want["candidates"][0], json!("orders"), "{want}");

    let count_err = client
        .tools_call("host.table.count", json!({"table": "staging_orders"}))
        .await
        .expect_err("count: staging_orders is not declared");
    let chart_err = client
        .tools_call("host.table.chart", json!({"sql": sql}))
        .await
        .expect_err("chart: staging_orders is not declared");
    for (tool, err) in [("count", &count_err), ("chart", &chart_err)] {
        assert_eq!(err.code, -32602, "{tool}: {err:?}");
        assert_eq!(err.error_code.as_deref(), Some("table_not_found"), "{tool}: {err:?}");
        assert_eq!(err.message, query_err.message, "{tool} message");
        assert_eq!(diagnosis_block(&err.data), want, "{tool} data");
        assert!(!err.message.contains("internal error") && !err.message.contains("request_id"), "{tool}: {}", err.message);
    }

    let log = extract_structured(&client.tools_call("host.table.query_log", json!({"limit": 5})).await.expect("query_log"));
    let log_id = log["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["sql"] == json!(sql)))
        .and_then(|r| r["id"].as_i64())
        .expect("the failed query is in the log");
    let diagnosed = extract_structured(
        &client
            .tools_call("host.table.query_diagnose", json!({"log_id": log_id}))
            .await
            .expect("query_diagnose is a success result"),
    );
    // The response-wide `data.quota` decoration shares the same `data` object.
    assert_eq!(diagnosis_block(&diagnosed["data"]), want, "query_diagnose data block: {diagnosed}");
}
