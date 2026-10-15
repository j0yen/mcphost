//! PRD-mcphost-docs-telemetry-failed-after-search AC3 (P0) — Given an
//! anonymous `host.help.search` followed by an anonymous failure, When read,
//! Then the row has `tenant_id NULL` and `failed_code NULL`; Given a search
//! followed by a failure 121 s later, Then `failed_code` stays NULL.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::state::now_unix;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn dtf_ac03_anonymous_search_row_has_null_tenant_and_is_never_joined() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    client.tools_call("host.help.search", json!({"q": "chain mappings"})).await.expect("anonymous search");
    // An anonymous failure: an unknown tool name.
    client.tools_call("nope.nothing", json!({})).await.expect_err("anonymous call to an unknown tool fails");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let rows = server.state.db.help_search_events_for_test().await.expect("rows");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, None, "anonymous search stores tenant_id NULL");
    assert_eq!(rows[0].3, None, "anonymous failure never joins");
}

#[tokio::test]
async fn dtf_ac03_failure_121_seconds_after_the_search_stays_unjoined() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "DTF AC3").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    server
        .state
        .db
        .record_help_search(Some(tenant.id), "old query".into(), None, now_unix() - 121)
        .await
        .expect("seed a 121-s-old search");

    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "bad", "kind": "echo", "spec": {"schema": 5}}))
        .await
        .expect_err("failing publish");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let rows = server.state.db.help_search_events_for_test().await.expect("rows");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].3, None, "a search older than 120 s is never joined: {rows:?}");
}
