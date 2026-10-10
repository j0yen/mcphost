//! AC1 — Given a keyed tenant and a published tool with an integer
//! argument, When the tenant calls it with `4.78`, Then the response code is
//! `args_invalid` and `calls` holds one new row for that tenant with
//! `ok = 0`, `error_class = refused`, `outcome = refused`,
//! `error_code = args_invalid`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn args_invalid_call_writes_one_refused_row() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Refused Args Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "intool",
                "kind": "echo",
                "spec": {"schema": {
                    "type": "object",
                    "properties": {"n": {"type": "integer"}},
                    "required": ["n"]
                }}
            }),
        )
        .await
        .expect("publish");

    let err = client
        .tools_call(&format!("{ns}.intool"), json!({"n": 4.78}))
        .await
        .expect_err("4.78 is not an integer");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let rows: Vec<(String, i64, String, String, String)> = conn
        .prepare(
            "SELECT tool_name, ok, error_class, outcome, error_code FROM calls \
             WHERE tenant_id = (SELECT id FROM tenants WHERE namespace = ?1)",
        )
        .expect("prepare")
        .query_map([&ns], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("collect");
    assert_eq!(
        rows,
        vec![(
            "intool".to_string(),
            0,
            "refused".to_string(),
            "refused".to_string(),
            "args_invalid".to_string()
        )]
    );
}
