//! PRD-mcphost-table-query-names-missing-table
//! AC6 -- Given `errors.json`, When the test lists the INVALID_PARAMS arm,
//! Then `table_column_not_found` is present with family Table, class
//! AgentFault, `retryable: false`, and `ERROR_CODES` is regenerated from
//! `error_registry` rather than hand-edited.
//!
//! "Family Table" is the registry's `storage` family: there is no separate
//! `table` family, and `table_not_found` / `table_row_conflict` (the code's
//! siblings) sit in `storage` too.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::error_registry::{ERROR_CODES, ErrorClass, Family, lookup};
use mcphost::errors::AppError;
use serde_json::{Value, json};

const CODE: &str = "table_column_not_found";

#[tokio::test]
async fn tqnf_ac06() {
    let row = lookup(CODE).expect("declared in the registry");
    assert_eq!(row.class, ErrorClass::AgentFault);
    assert_eq!(row.family, lookup("table_not_found").expect("sibling").family);
    assert_eq!(row.family, Family::Storage);
    assert!(!row.retryable);
    assert!(!row.remedy.is_empty() && row.remedy.len() <= 200, "{}", row.remedy);
    assert_eq!(ERROR_CODES.iter().filter(|e| e.code == CODE).count(), 1);
    // Declared through `err_code!` at its emit site, not a hand row.
    assert!(row.site.starts_with("src/tables.rs:"), "{}", row.site);
    assert_eq!(mcphost::tables::TABLE_COLUMN_NOT_FOUND, CODE);
    let registry_src =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/error_registry.rs"))
            .expect("read");
    assert!(!registry_src.contains(CODE), "ERROR_CODES must come from err_code!, not a hand row");

    // The INVALID_PARAMS arm: the same bucket as `table_not_found`.
    let class_of = |code: &'static str| {
        AppError::Structured { code, message: String::new(), data: Value::Null }.into_error_data().code
    };
    assert_eq!(class_of(mcphost::tables::TABLE_COLUMN_NOT_FOUND), rmcp::model::ErrorCode::INVALID_PARAMS);
    assert_eq!(class_of(mcphost::tables::TABLE_COLUMN_NOT_FOUND), class_of("table_not_found"));

    let server = TestServer::start().await;
    let catalog: Value = reqwest::Client::new()
        .get(format!("{}/errors.json", server.base_url))
        .send()
        .await
        .expect("GET /errors.json")
        .json()
        .await
        .expect("JSON");
    let listed = catalog["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .find(|r| r["code"] == CODE)
        .expect("/errors.json lists the code");
    assert_eq!(listed["class"], "agent_fault");
    assert_eq!(listed["family"], "storage");
    assert_eq!(listed["retryable"], false);
    assert_eq!(listed["remedy"], row.remedy);

    let one = extract_structured(
        &McpClient::new(&server.base_url).tools_call("host.errors", json!({"code": CODE})).await.expect("host.errors"),
    );
    assert_eq!(one["code"], CODE);
}
