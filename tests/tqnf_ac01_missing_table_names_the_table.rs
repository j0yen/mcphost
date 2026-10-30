//! PRD-mcphost-table-query-names-missing-table
//! AC1 -- Given a tenant with tables `orders` and `customers` declared, When
//! `host.table.query` runs `SELECT * FROM staging_orders`, Then the JSON-RPC
//! error code is `-32602`, `data.error_code` is `table_not_found`,
//! `data.table` is `staging_orders`, `data.candidates[0]` is `orders`,
//! `data.hint` is a non-empty string, and the message does not contain
//! `internal error` or `request_id`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tqnf_ac01() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tqnf AC1 Tenant").await;
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
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM staging_orders"}))
        .await
        .expect_err("staging_orders is not declared");
    assert_eq!(err.code, -32602, "{err:?}");
    assert_eq!(err.error_code.as_deref(), Some("table_not_found"), "{err:?}");
    assert_eq!(err.data["table"], json!("staging_orders"), "{err:?}");
    assert_eq!(err.data["candidates"][0], json!("orders"), "{err:?}");
    assert!(err.data["hint"].as_str().is_some_and(|h| !h.is_empty()), "hint: {:?}", err.data);
    assert_eq!(err.data["declared_tables_count"], json!(2), "{err:?}");
    assert!(!err.message.contains("internal error"), "{}", err.message);
    assert!(!err.message.contains("request_id"), "{}", err.message);
    assert!(err.message.starts_with("no table 'staging_orders' is declared for this tenant"), "{}", err.message);
}
