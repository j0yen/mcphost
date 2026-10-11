//! PRD-mcphost-dry-run-on-write-verbs
//! AC5 -- Given `host.table.drop {name: "nope", dry_run: true}`, When called,
//! Then the error is the unknown-table error (`table_not_found`) every other
//! verb gives that name; Given a tenant at `table_tables_max` calling
//! `host.table.create {dry_run: true}`, Then the error is the real
//! `quota_exceeded`, the same one the call gives without `dry_run`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn dry_run_returns_the_real_not_found_and_the_real_quota_error() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let missing = client
        .tools_call("host.table.drop", json!({"name": "nope", "dry_run": true}))
        .await
        .expect_err("dry-run drop of an unknown table fails");
    assert_eq!(missing.error_code.as_deref(), Some("table_not_found"), "{missing:?}");
    assert!(missing.message.contains("nope"), "{missing:?}");

    // The free plan's table_tables_max is 5 (src/plans.rs).
    for i in 0..5 {
        client
            .tools_call("host.table.create", json!({"name": format!("t{i}"), "columns": {"x": "text"}}))
            .await
            .unwrap_or_else(|e| panic!("create t{i}: {e:?}"));
    }

    let real = client
        .tools_call("host.table.create", json!({"name": "t5", "columns": {"x": "text"}}))
        .await
        .expect_err("sixth table exceeds the quota");
    assert_eq!(real.error_code.as_deref(), Some("quota_exceeded"), "{real:?}");

    let dry = client
        .tools_call("host.table.create", json!({"name": "t5", "columns": {"x": "text"}, "dry_run": true}))
        .await
        .expect_err("dry run fails exactly where the real call fails");
    assert_eq!(dry.error_code.as_deref(), Some("quota_exceeded"), "{dry:?}");
    assert_eq!(dry.message, real.message);

    // Under quota a dry-run create succeeds and leaves no table behind.
    client
        .tools_call("host.table.drop", json!({"name": "t4"}))
        .await
        .expect("drop t4");
    let would = client
        .tools_call("host.table.create", json!({"name": "t5", "columns": {"x": "text"}, "dry_run": true}))
        .await
        .expect("dry-run create under quota");
    assert_eq!(common::extract_structured(&would)["_meta"]["dry_run"], json!(true));
    let listed = common::extract_structured(&client.tools_call("host.table.list", json!({})).await.expect("list"));
    let names: Vec<&str> = listed["tables"]
        .as_array()
        .expect("tables")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(!names.contains(&"t5"), "a dry-run create leaves nothing: {names:?}");
}
