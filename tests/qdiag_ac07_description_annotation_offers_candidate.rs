//! PRD-mcphost-query-diagnosis
//! AC7 — Given a column with a `description` annotation "amount in USD",
//! When `SELECT usd FROM expenses` is refused, Then the diagnosis offers
//! `amount` as a candidate through the annotation text.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn description_annotation_word_offers_the_owning_column_as_a_candidate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");
    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "column": "amount", "key": "description", "value": "amount in USD"}),
        )
        .await
        .expect("model_set");

    let err = client
        .tools_call("host.table.query", json!({"sql": "SELECT usd FROM expenses"}))
        .await
        .expect_err("usd is not a declared column");
    assert_eq!(err.error_code.as_deref(), Some("storage"), "err: {err:?}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["entries"][0];
    let identifier = &entry["diagnosis"]["identifiers"][0];
    assert_eq!(identifier["term"], "usd", "entry: {entry}");
    assert_eq!(identifier["matched_to"], "amount", "entry: {entry}");

    let hint = entry["hint"].as_str().expect("hint");
    assert!(hint.contains("amount"), "{hint}");
}
