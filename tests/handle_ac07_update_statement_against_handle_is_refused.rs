//! PRD-mcphost-result-handles
//! AC7 -- Given an `UPDATE hdl_<id> ...` statement, When submitted to
//! `host.table.query`, Then it is refused by the existing read-only guard
//! and the handle is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn update_against_a_handle_is_refused_structurally() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ResultHandles AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "events", "columns": {"n": "integer"}}))
        .await
        .expect("create events");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "events", "rows": [{"n": 1}, {"n": 2}, {"n": 3}]}),
        )
        .await
        .expect("append");

    let materialize = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM events", "handle": true}))
            .await
            .expect("materialize"),
    );
    let handle = materialize["handle"].as_str().expect("handle").to_string();

    let err = client
        .tools_call("host.table.query", json!({"sql": format!("UPDATE {handle} SET n = 99")}))
        .await
        .expect_err("UPDATE against a handle must be refused");
    assert_eq!(err.error_code.as_deref(), Some("table_query_rejected"), "err: {err:?}");

    // The handle's own rows are unchanged.
    let after = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": format!("SELECT n FROM {handle} ORDER BY n")}))
            .await
            .expect("query handle after refused UPDATE"),
    );
    let values: Vec<i64> = after["rows"].as_array().unwrap().iter().map(|r| r["n"].as_i64().unwrap()).collect();
    assert_eq!(values, vec![1, 2, 3], "handle must be unchanged: {after}");
}
