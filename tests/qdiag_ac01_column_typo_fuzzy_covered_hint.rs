//! PRD-mcphost-query-diagnosis
//! AC1 — Given table `expenses` with column `amount`, When `SELECT amout
//! FROM expenses` is submitted, Then the query is refused, the log row's
//! `diagnosis` marks `amout` as `fuzzy_covered` matched to `amount` with
//! similarity >= 0.9, and `hint` names both.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn column_typo_is_fuzzy_covered_and_hinted() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");

    let err = client
        .tools_call("host.table.query", json!({"sql": "SELECT amout FROM expenses"}))
        .await
        .expect_err("amout is not a declared column");
    assert_eq!(err.error_code.as_deref(), Some("storage"), "err: {err:?}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["rows"][0];
    let diag = &entry["diagnosis"];
    let identifier = &diag["identifiers"][0];
    assert_eq!(identifier["term"], "amout", "diag: {diag}");
    assert_eq!(identifier["kind"], "column", "diag: {diag}");
    assert_eq!(identifier["status"], "fuzzy_covered", "diag: {diag}");
    assert_eq!(identifier["matched_to"], "amount", "diag: {diag}");
    let similarity = identifier["top_candidates"][0]["similarity"].as_f64().expect("similarity");
    assert!(similarity >= 0.9, "similarity {similarity} must be >= 0.9: {diag}");

    let hint = entry["hint"].as_str().expect("hint");
    assert!(hint.contains("amout"), "{hint}");
    assert!(hint.contains("amount"), "{hint}");
}
