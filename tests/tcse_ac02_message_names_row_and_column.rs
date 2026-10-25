//! PRD-mcphost-table-conflict-structured-error
//! AC2 -- Given the AC1 error, When its message is read, Then it matches
//! `row 1 conflicts on seq (primary_key) in table <name>` and contains no
//! `constraint failed`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tcse_ac02() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tcse AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {"seq": "integer"}, "primary_key": "seq"}),
        )
        .await
        .expect("table create ok");
    client
        .tools_call("host.table.append", json!({"table": "events", "rows": [{"seq": 1}]}))
        .await
        .expect("first append ok");

    let err = client
        .tools_call("host.table.append", json!({"table": "events", "rows": [{"seq": 2}, {"seq": 1}]}))
        .await
        .expect_err("duplicate primary key must fail");
    assert_eq!(err.message, "row 1 conflicts on seq (primary_key) in table events", "{err:?}");
    assert!(!err.message.contains("constraint failed"), "{err:?}");
}
