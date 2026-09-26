//! PRD-mcphost-shared-tool-caller-usage
//! AC5 (P0) — Given 1 500 distinct end users in a window, When
//! `host.usage {by: "end_user", limit: 1000}` runs, Then 1 000 rows return
//! with a cursor and the next page returns the remaining 500.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const TOTAL_USERS: i64 = 1500;

#[tokio::test]
async fn end_user_breakdown_pages_past_the_1000_row_cap() {
    let server = TestServer::start().await;
    let (ns_o, key_o) = signup(&server.base_url, "Owner O").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_t", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes T");
    let owner = server
        .state
        .db
        .find_tenant_by_namespace(ns_o.clone())
        .await
        .expect("db query")
        .expect("owner exists");

    let now = mcphost::state::now_unix();
    for i in 0..TOTAL_USERS {
        server
            .state
            .db
            .insert_call_row_for_test_full(
                owner.id,
                "tool_t".to_string(),
                None,
                Some(format!("u{i:04}")),
                true,
                10,
                now,
            )
            .await
            .expect("seed end-user call");
    }

    let page1 = client_o
        .tools_call(
            "host.usage",
            json!({"by": "end_user", "window": "1d", "limit": 1000}),
        )
        .await
        .expect("host.usage page 1");
    let page1 = extract_structured(&page1);
    let rows1 = page1["rows"].as_array().expect("rows array");
    assert_eq!(rows1.len(), 1000, "page 1 must return exactly 1000 rows");
    let cursor = page1["cursor"]
        .as_str()
        .expect("page 1 must carry a cursor")
        .to_string();

    let page2 = client_o
        .tools_call(
            "host.usage",
            json!({"by": "end_user", "window": "1d", "limit": 1000, "cursor": cursor}),
        )
        .await
        .expect("host.usage page 2");
    let page2 = extract_structured(&page2);
    let rows2 = page2["rows"].as_array().expect("rows array");
    assert_eq!(
        rows2.len(),
        (TOTAL_USERS - 1000) as usize,
        "page 2 must return the remaining 500 rows"
    );
    assert!(
        page2["cursor"].is_null(),
        "page 2 must be the last page: {:?}",
        page2["cursor"]
    );

    // No overlap between the two pages' keys.
    let keys1: std::collections::BTreeSet<&str> =
        rows1.iter().map(|r| r["key"].as_str().unwrap()).collect();
    let keys2: std::collections::BTreeSet<&str> =
        rows2.iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert!(
        keys1.is_disjoint(&keys2),
        "the two pages must not share any key"
    );
}
