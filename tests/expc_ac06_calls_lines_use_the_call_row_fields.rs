//! PRD-mcphost-export-data-class-completeness-v2
//! AC6 -- Given `calllog::CallRow`, When the test compares the keys of a
//! `calls.jsonl` line to `CallRow::FIELDS` and to `paged::CallsList::FIELDS`,
//! Then they are equal.

use crate::common;
use crate::expc;

use std::collections::BTreeSet;

use common::TestServer;
use mcphost::calllog::CallRow;
use mcphost::paged::{CallsList, Paged};
use serde_json::json;

#[tokio::test]
async fn expc_ac06() {
    let server = TestServer::start().await;
    let tenant = expc::new_tenant(&server, "Expc AC6 Tenant").await;
    tenant.make_calls(3).await;
    let (_run, entries) = tenant.export(&server, json!({})).await;

    let calls = expc::lines(&entries["calls.jsonl"]);
    assert_eq!(calls.len(), 3, "{calls:?}");

    let row_fields: BTreeSet<&str> = CallRow::FIELDS.iter().copied().collect();
    let list_fields: BTreeSet<&str> = CallsList::FIELDS.iter().copied().collect();
    assert_eq!(row_fields, list_fields, "CallRow::FIELDS and CallsList::FIELDS name the same keys");
    for line in &calls {
        let keys: BTreeSet<&str> = line.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(keys, row_fields, "a calls.jsonl line carries exactly CallRow::FIELDS: {line}");
    }
}
