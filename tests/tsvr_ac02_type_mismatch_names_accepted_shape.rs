//! PRD-mcphost-table-schema-violation-names-the-remedy
//! AC2 — Given a table declared `(ts timestamp, amount real)`, When a row
//! sends `{"ts": 1760000000}` (and separately `{"amount": "3"}`), Then the
//! message names the declared type, the accepted shape and the JSON kind
//! sent, `data.expected.accepts` is non-empty, `data.got` names the kind,
//! and every `ColumnType` variant has a non-empty `accepts` whose example
//! value passes `matches` (drift test).

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::tables::ColumnType;
use serde_json::json;

const ALL: [ColumnType; 6] = [
    ColumnType::Text,
    ColumnType::Integer,
    ColumnType::Real,
    ColumnType::Timestamp,
    ColumnType::Boolean,
    ColumnType::Json,
];

#[test]
fn every_column_type_has_accepts_and_a_matching_example() {
    // Exhaustive match: a new variant fails to compile here until listed.
    for ty in ALL {
        match ty {
            ColumnType::Text
            | ColumnType::Integer
            | ColumnType::Real
            | ColumnType::Timestamp
            | ColumnType::Boolean
            | ColumnType::Json => {}
        }
        assert!(!ty.accepts().is_empty(), "{} has no accepts", ty.as_str());
        assert!(ty.accepts().iter().all(|s| !s.is_empty()));
        assert!(ty.matches(&ty.example()), "{} example does not match", ty.as_str());
    }
}

#[tokio::test]
async fn type_mismatch_names_type_accepted_shape_and_got() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TSVR AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {"ts": "timestamp", "amount": "real"}}),
        )
        .await
        .expect("table create");

    let err = client
        .tools_call("host.table.append", json!({"table": "events", "rows": [{"ts": 1760000000}]}))
        .await
        .expect_err("epoch number into a timestamp column must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("table_schema_violation"), "{err:?}");
    assert!(
        err.message.contains(
            "column 'ts' must be of type 'timestamp' (accepts an RFC 3339 string); got number"
        ),
        "{}",
        err.message
    );
    assert_eq!(err.data["kind"], json!("type_mismatch"));
    assert_eq!(err.data["expected"]["type"], json!("timestamp"));
    assert_eq!(err.data["expected"]["accepts"], json!(["an RFC 3339 string"]));
    assert_eq!(err.data["got"], json!("number"));

    let err = client
        .tools_call("host.table.append", json!({"table": "events", "rows": [{"amount": "abc"}]}))
        .await
        .expect_err("string into a real column must be rejected");
    assert!(
        err.message.contains("column 'amount' must be of type 'real' (accepts a number); got string"),
        "{}",
        err.message
    );
    assert!(!err.data["expected"]["accepts"].as_array().expect("accepts").is_empty());
    assert_eq!(err.data["got"], json!("string"));
}
