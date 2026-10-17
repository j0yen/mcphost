//! PRD-mcphost-same-user-second-agent
//! AC2 — Given a join code with `max_uses: 1` already used, When a third
//! connection uses it, Then the error is `join_code_invalid` with
//! `next_call.tool == "host.whoami"` and no `tenant_clients` row is inserted.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sibl_ac02_used_up_join_code_is_refused() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Sibling Host").await;
    let first = McpClient::with_bearer(&server.base_url, &key);
    let me = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let path = me["join_url"].as_str().expect("join_url").trim_start_matches(&server.base_url).to_string();

    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    server.state.db.set_join_code_max_uses(tenant.id, 1).await.expect("limit the code to one use");

    // The one allowed use.
    let second = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    second.tools_call("host.whoami", json!({})).await.expect("the one allowed join");
    let rows_before = server.state.db.list_live_clients(tenant.id).await.expect("clients").len();
    assert_eq!(rows_before, 2);

    // The third connection is refused.
    let third = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let err = third
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a used-up join code must be refused");
    assert_eq!(err.error_code.as_deref(), Some("join_code_invalid"), "{err:?}");
    assert_eq!(err.data["next_call"]["tool"], json!("host.whoami"), "{err:?}");

    let rows_after = server.state.db.list_live_clients(tenant.id).await.expect("clients").len();
    assert_eq!(rows_after, rows_before, "no tenant_clients row is inserted for the refused join");
    let (all_rows,): (i64,) = {
        let db = server.state.db.clone();
        (db.count_tenant_clients_since(0).await.expect("count"),)
    };
    assert_eq!(all_rows, 1, "exactly the one sibling row exists beside client 0");
}
