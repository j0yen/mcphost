//! PRD-mcphost-docs-telemetry-failed-after-search AC2 (P0) — Given a keyed
//! tenant that calls `host.help.search` and then a failing
//! `host.tool_publish`, When the events table is read, Then one row carries
//! the normalized query, `failed_code = "invalid_spec"` and
//! `failed_tool = "host.tool_publish"`; a second failure leaves it unchanged.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;
use std::time::Duration;

async fn failing_publish(client: &McpClient) {
    let err = client
        .tools_call("host.tool_publish", json!({"name": "bad", "kind": "echo", "spec": {"schema": 5}}))
        .await
        .expect_err("declaring both upstream and method/url must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
}

#[tokio::test]
async fn dtf_ac02_search_then_failure_joins_one_row_once() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "DTF AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.help.search", json!({"q": "publish python tool t_8117b3bb at https://x.y/z 42"}))
        .await
        .expect("host.help.search");
    failing_publish(&client).await;

    let mut rows = Vec::new();
    for _ in 0..100 {
        rows = server.state.db.help_search_events_for_test().await.expect("rows");
        if rows.first().is_some_and(|r| r.3.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (tenant_id, query_norm, _anchor, code, tool) = rows[0].clone();
    assert!(tenant_id.is_some(), "keyed search stores its tenant");
    assert_eq!(query_norm, "publish python tool <tenant> at <url> <n>");
    assert_eq!(code.as_deref(), Some("invalid_spec"));
    assert_eq!(tool.as_deref(), Some("host.tool_publish"));

    // A second failure: the row is already joined, so it never changes.
    failing_publish(&client).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = server.state.db.help_search_events_for_test().await.expect("rows");
    assert_eq!(after, rows, "a second failure must not touch the joined row");
}

#[test]
fn dtf_ac02_normalize_query_table_pins_twelve_inputs() {
    let table: [(&str, &str); 12] = [
        ("publish python tool t_8117b3bb at https://x.y/z 42", "publish python tool <tenant> at <url> <n>"),
        ("Chain PREV Result", "chain prev result"),
        ("  lots   of\tspace  ", "lots of space"),
        ("\"quoted\" 'words' `here`", "quoted words here"),
        ("see http://a.b/c?d=1 now", "see <url> now"),
        ("python3 import", "python<n> import"),
        ("limit 1000 rows 5", "limit <n> rows <n>"),
        ("t_deadbeef and t_00ff", "<tenant> and <tenant>"),
        ("my key mcp_0123456789abcdef0123456789 broke", "my key <key> broke"),
        ("", ""),
        ("https://x.y", "<url>"),
        (&"abcdefghij ".repeat(20), "abcdefghij abcdefghij abcdefghij abcdefghij abcdefghij abcdefghij abcdefghij abc"),
    ];
    for (input, want) in table {
        assert_eq!(mcphost::help_search::normalize_query(input), want, "input {input:?}");
    }
}
