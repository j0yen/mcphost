//! PRD-mcphost-csv-to-table-one-call
//! AC5 -- Given a free tenant and a 300,000-row CSV, When loaded, Then the
//! refusal is `quota_table_rows {limit, used, found: 300000}`, no table is
//! created, and `host.quickstart.limits.plan.table_rows_max` shows the same
//! limit.
//!
//! Driven over the real MCP surface (signup -> host.docs.put ->
//! host.table.from_blob), so the wire shape of the refusal is what is asserted.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

const ROWS: usize = 300_000;

#[tokio::test]
async fn free_tenant_300k_rows_is_refused_with_limit_used_found_and_creates_no_table() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "csvtab AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // One short column: 300,000 rows stay well under the 2 MiB document cap.
    let mut csv = String::from("n\n");
    for _ in 0..ROWS {
        csv.push_str("1\n");
    }
    client
        .tools_call("host.docs.put", json!({"name": "big.csv", "mime": "text/csv", "content": csv}))
        .await
        .expect("host.docs.put ok");

    let err = client
        .tools_call("host.table.from_blob", json!({"doc": "big.csv", "name": "big"}))
        .await
        .expect_err("300,000 rows exceed the free plan");
    assert_eq!(err.error_code.as_deref(), Some("quota_table_rows"), "{err:?}");
    assert_eq!(err.data["found"], json!(ROWS), "found: {err:?}");
    assert_eq!(err.data["used"], json!(0), "used: {err:?}");
    let limit = err.data["limit"].clone();
    assert!(limit.is_i64() || limit.is_u64(), "limit must be a number: {err:?}");

    // The same number the quickstart reports as the plan's ceiling.
    let qs = client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart");
    let structured = common::extract_structured(&qs);
    assert_eq!(structured["limits"]["plan"]["table_rows_max"], limit, "limits.plan.table_rows_max");

    // Nothing was created.
    let list = client.tools_call("host.table.list", json!({})).await.expect("host.table.list");
    let listed = common::extract_structured(&list).to_string();
    assert!(!listed.contains("\"big\""), "no table may exist after a refused load: {listed}");
}
