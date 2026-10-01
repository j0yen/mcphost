//! PRD-mcphost-query-diagnosis
//! AC2 — Given no table named `expense`, When `SELECT * FROM expense` is
//! submitted, Then the diagnosis marks the table `fuzzy_covered` to
//! `expenses` and the hint says so.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn table_typo_is_fuzzy_covered_and_hinted() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");

    let err = client
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM expense"}))
        .await
        .expect_err("expense (singular) is not declared");
    assert_eq!(err.error_code.as_deref(), Some("storage"), "err: {err:?}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["entries"][0];
    let identifier = &entry["diagnosis"]["identifiers"][0];
    assert_eq!(identifier["term"], "expense", "entry: {entry}");
    assert_eq!(identifier["kind"], "table", "entry: {entry}");
    assert_eq!(identifier["status"], "fuzzy_covered", "entry: {entry}");
    assert_eq!(identifier["matched_to"], "expenses", "entry: {entry}");

    let hint = entry["hint"].as_str().expect("hint");
    assert!(hint.contains("expense"), "{hint}");
    assert!(hint.contains("expenses"), "{hint}");
}
