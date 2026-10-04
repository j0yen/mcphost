//! PRD-mcphost-result-handles
//! AC7 -- Given an `UPDATE hdl_<id> ...` statement, When submitted to
//! `host.table.query`, Then it is refused by the existing read-only guard
//! and the handle is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn update_on_a_handle_is_refused_and_leaves_it_unchanged() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Handle AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "readings", "columns": {"v": "real"}}))
        .await
        .expect("create readings");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "readings", "rows": [{"v": 1.0}, {"v": 2.0}, {"v": 3.0}]}),
        )
        .await
        .expect("append readings");

    let summary = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM readings", "handle": true}))
            .await
            .expect("materialize handle"),
    );
    let handle = summary["handle"].as_str().expect("handle name").to_string();

    let err = client
        .tools_call("host.table.query", json!({"sql": format!("UPDATE {handle} SET v = 99.0")}))
        .await
        .expect_err("an UPDATE against a handle must be refused, same as against a declared table");
    assert_eq!(err.error_code.as_deref(), Some("table_query_rejected"), "error: {err:?}");

    // Unchanged: the same aggregate as before the attempted UPDATE.
    let after = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": format!("SELECT SUM(v) AS s FROM {handle}")}))
            .await
            .expect("read handle after refused UPDATE"),
    );
    assert_eq!(after["rows"][0]["s"], 6.0, "handle must be unchanged: {after}");
}
