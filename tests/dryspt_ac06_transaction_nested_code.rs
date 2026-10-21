//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs
//! AC6 -- the bundled SQLite's `BEGIN`-inside-a-transaction refusal converts
//! to the non-retryable `transaction_nested` host-fault code; any other
//! `SqliteFailure` with code 1 stays `storage`.

use crate::common;
use common::TestServer;
use mcphost::errors::{AppError, HOST_FAULT_CODES, NESTED_TRANSACTION_SQLITE_MESSAGE};
use rusqlite::Connection;
use serde_json::Value;

fn data_of(err: AppError) -> (String, Value) {
    let d = err.into_error_data();
    (d.message.to_string(), d.data.expect("error data"))
}

#[tokio::test]
async fn dryspt_ac06() {
    // Produced by the bundled SQLite itself: a SQLite upgrade that rewords
    // the message fails here instead of silently dropping the mapping.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("BEGIN").unwrap();
    let e = conn.execute_batch("BEGIN").expect_err("second BEGIN is refused");
    match &e {
        rusqlite::Error::SqliteFailure(f, Some(msg)) => {
            assert_eq!(f.extended_code, 1, "SQLITE_ERROR");
            assert_eq!(msg, NESTED_TRANSACTION_SQLITE_MESSAGE);
            assert_eq!(msg, "cannot start a transaction within a transaction");
        }
        other => panic!("unexpected error shape: {other:?}"),
    }

    let converted = AppError::from(e);
    let (message, data) = data_of(converted);
    assert_eq!(data["error_code"], "transaction_nested", "{data}");
    assert_eq!(data["retry"]["retryable"], false, "{data}");
    assert!(data["request_id"].as_str().is_some_and(|s| !s.is_empty()), "request_id is set: {data}");
    assert!(!message.to_lowercase().contains("sqlite") && !message.contains("within a transaction"), "no SQLite text: {message}");
    assert!(HOST_FAULT_CODES.contains(&"transaction_nested"));

    // Same code, different text: stays `storage`.
    let other = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error { code: rusqlite::ErrorCode::Unknown, extended_code: 1 },
        Some("near \"x\": syntax error".to_string()),
    );
    let (_, data) = data_of(AppError::from(other));
    assert_eq!(data["error_code"], "storage", "{data}");

    // `/errors.json` lists it.
    let server = TestServer::start().await;
    let body: Value = reqwest::Client::new()
        .get(format!("{}/errors.json", server.base_url))
        .send()
        .await
        .expect("GET /errors.json")
        .json()
        .await
        .expect("json");
    let row = body["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .find(|r| r["code"] == "transaction_nested")
        .expect("/errors.json lists transaction_nested");
    assert_eq!(row["retryable"], false, "{row}");
    assert_eq!(row["family"], "storage", "{row}");
}
