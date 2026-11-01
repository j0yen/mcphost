//! PRD-mcphost-table-conflict-structured-error
//! AC1 -- Given a table created with `primary_key: "seq"` and one row
//! `{seq: 1}`, When `host.table.append` is called with `[{seq: 2}, {seq: 1}]`,
//! Then the code is `table_row_conflict`, `data` carries `{table, column,
//! constraint, row_index}`, the JSON-RPC code is `-32602`, and
//! `host.table.count` still returns 1.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tcse_ac01() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tcse AC1 Tenant").await;
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
    assert_eq!(err.error_code.as_deref(), Some("table_row_conflict"), "{err:?}");
    assert_eq!(err.code, -32602, "{err:?}");
    for (k, v) in [
        ("table", json!("events")),
        ("column", json!("seq")),
        ("constraint", json!("primary_key")),
        ("row_index", json!(1)),
    ] {
        assert_eq!(err.data[k], v, "data.{k}: {:?}", err.data);
    }

    let count = client
        .tools_call("host.table.count", json!({"table": "events"}))
        .await
        .expect("count ok");
    assert_eq!(extract_structured(&count)["count"], json!(1), "nothing may be appended");
}
