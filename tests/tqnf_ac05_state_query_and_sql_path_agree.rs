//! PRD-mcphost-table-query-names-missing-table
//! AC5 -- Given `host.state.query {"table": "staging_orders"}` on the same
//! tenant, When it runs, Then its `error_code` and numeric code equal AC 1's,
//! and its message equals AC 1's message with everything from the first `;`
//! onward removed (the two paths agree on the sentence; only the SQL path
//! adds the `; closest:` suffix).
//!
//! The PRD's premise that `host.state.query` answers `table_not_found` does
//! not hold at the cited tree: it answers the registered `state_table_not_found`
//! (same family, class, remedy and `-32602`), and the PRD's own non-goal and
//! migration note keep `host.state.*` unchanged (tests/state_ac08 and
//! tests/whin_ac07 pin the code). So the code is compared as the same
//! `table_not_found` under the `state_` namespace the state verbs prefix to
//! every code they own.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tqnf_ac05() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tqnf AC5 Tenant").await;
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

    let sql_err = client
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM staging_orders"}))
        .await
        .expect_err("sql path: staging_orders is not declared");
    let state_err = client
        .tools_call("host.state.query", json!({"table": "staging_orders"}))
        .await
        .expect_err("state path: staging_orders is not declared");

    assert_eq!(sql_err.error_code.as_deref(), Some("table_not_found"), "{sql_err:?}");
    assert_eq!(
        state_err.error_code.as_deref(),
        Some("state_table_not_found"),
        "state {state_err:?} vs sql {sql_err:?}"
    );
    assert_eq!(
        state_err.error_code.as_deref().and_then(|c| c.strip_prefix("state_")),
        sql_err.error_code.as_deref(),
        "the two paths name the same refusal"
    );
    assert_eq!(state_err.code, sql_err.code, "state {state_err:?} vs sql {sql_err:?}");
    let sentence = |m: &str| m.split(';').next().unwrap_or_default().to_string();
    assert_eq!(state_err.message, sentence(&sql_err.message), "state {state_err:?} vs sql {sql_err:?}");
    assert!(!state_err.message.contains(';'), "{}", state_err.message);
}
