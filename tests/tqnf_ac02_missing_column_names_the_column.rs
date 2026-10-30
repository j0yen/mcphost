//! PRD-mcphost-table-query-names-missing-table
//! AC2 -- Given a tenant with tables `orders` and `customers` declared, When
//! `host.table.query` runs `SELECT totl FROM orders`, Then `data.error_code`
//! is `table_column_not_found`, `data.column` is `totl`, `data.table` is
//! `orders`, and `data.candidates[0]` is `total` (the declared column).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tqnf_ac02() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tqnf AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for (name, columns) in [
        ("orders", json!({"id": "integer", "total": "real", "status": "text"})),
        ("customers", json!({"id": "integer", "email": "text"})),
    ] {
        client
            .tools_call("host.table.create", json!({"name": name, "columns": columns}))
            .await
            .expect("create");
    }

    let err = client
        .tools_call("host.table.query", json!({"sql": "SELECT totl FROM orders"}))
        .await
        .expect_err("totl is not a declared column");
    assert_eq!(err.code, -32602, "{err:?}");
    assert_eq!(err.error_code.as_deref(), Some("table_column_not_found"), "{err:?}");
    assert_eq!(err.data["column"], json!("totl"), "{err:?}");
    assert_eq!(err.data["table"], json!("orders"), "{err:?}");
    assert_eq!(err.data["candidates"][0], json!("total"), "{err:?}");
    assert!(err.data["hint"].as_str().is_some_and(|h| !h.is_empty()), "hint: {:?}", err.data);
    assert_eq!(err.message, "no column 'totl' in table 'orders'; closest: total", "{}", err.message);
    assert!(!err.message.contains("internal error") && !err.message.contains("request_id"), "{}", err.message);
}
