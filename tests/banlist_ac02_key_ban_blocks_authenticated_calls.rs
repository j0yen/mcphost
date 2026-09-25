//! PRD-mcphost-abuse-guard-ban-list
//! AC2 (P0) — Given a banned tenant key, When any authenticated tool is
//! called with it, Then error `banned` returns, `hits` increments, and the
//! call is not recorded in `calls`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn banned_key_refuses_every_authenticated_call_and_writes_no_calls_row() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Soon Banned Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish before the ban");
    let qualified = format!("{tenant_ns}.hello");

    let ban = admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "key", "subject": key, "ttl": "24h", "reason": "abuse"}),
        )
        .await
        .expect("admin.ban.add");
    let ban_id = extract_structured(&ban)["id"].as_i64().expect("ban id");

    let err = client
        .tools_call(&qualified, json!({"n": 1}))
        .await
        .expect_err("a banned key's tool call must be refused");
    assert_eq!(err.error_code.as_deref(), Some("banned"));

    let err2 = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a banned key's host.* call must also be refused");
    assert_eq!(err2.error_code.as_deref(), Some("banned"));

    let list = admin
        .tools_call("admin.ban.list", json!({"active_only": true}))
        .await
        .expect("admin.ban.list");
    let rows = extract_structured(&list)["bans"].as_array().cloned().unwrap_or_default();
    let row = rows
        .iter()
        .find(|r| r["id"].as_i64() == Some(ban_id))
        .unwrap_or_else(|| panic!("ban {ban_id} missing from admin.ban.list: {rows:?}"));
    assert_eq!(row["hits"].as_i64(), Some(2), "both refused calls above must increment hits");

    let admin_usage = admin
        .tools_call("admin.usage", json!({"window": "24h"}))
        .await
        .expect("admin.usage");
    let usage_rows = extract_structured(&admin_usage)["usage"].as_array().cloned().unwrap_or_default();
    assert!(
        usage_rows
            .iter()
            .all(|r| !(r["tenant"] == json!(tenant_ns) && r["tool"] == json!("hello"))),
        "a refused call must not be recorded in calls: {usage_rows:?}"
    );
}
