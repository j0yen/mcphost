//! PRD-mcphost-transactions-only-via-helper AC5 -- `host.table.append` runs
//! for real, then with `dry_run: true`: the real rows are committed, the
//! dry-run rows are gone, neither call errors, and `insert_rows_sync` holds
//! no `is_autocommit` branch of its own (`with_tx` owns the nesting rule).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn count(client: &McpClient) -> i64 {
    let r = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM t1"}))
        .await
        .expect("count query");
    extract_structured(&r)["rows"][0]["n"].as_i64().expect("n")
}

#[tokio::test]
async fn txnhelp_ac05() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Txnhelp AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t1", "columns": {"a": "text"}}))
        .await
        .expect("create t1");

    client
        .tools_call("host.table.append", json!({"table": "t1", "rows": [{"a": "real-1"}, {"a": "real-2"}]}))
        .await
        .expect("real append does not error");
    assert_eq!(count(&client).await, 2, "the real rows are committed");

    let dry = client
        .tools_call(
            "host.table.append",
            json!({"table": "t1", "rows": [{"a": "dry-1"}, {"a": "dry-2"}, {"a": "dry-3"}], "dry_run": true}),
        )
        .await
        .expect("dry-run append does not error");
    assert_eq!(extract_structured(&dry)["_meta"]["dry_run"], json!(true), "{dry:?}");
    assert_eq!(count(&client).await, 2, "the dry-run rows are gone after the dry run");

    let gone = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM t1 WHERE a LIKE 'dry-%'"}))
        .await
        .expect("dry rows query");
    assert_eq!(extract_structured(&gone)["rows"][0]["n"], json!(0));

    // The hand-rolled nested branch is gone from `insert_rows_sync`.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tables.rs")).expect("read tables.rs");
    let start = src.find("fn insert_rows_sync(").expect("insert_rows_sync exists");
    let end = start + src[start..].find("\n}\n").expect("end of insert_rows_sync");
    let body = &src[start..end];
    assert!(!body.contains("is_autocommit"), "insert_rows_sync must not branch on is_autocommit");
    assert!(body.contains("with_tx("), "insert_rows_sync goes through with_tx");
}
