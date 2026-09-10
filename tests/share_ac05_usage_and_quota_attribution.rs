//! AC5 — Given B calls `geo` ten times, When usage is read, Then A's
//! `calls_by_others` shows `t_B: 10` and B's `calls_to_shared` shows 10;
//! B's `calls_per_day` counter increased by 10.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ten_cross_tenant_calls_are_attributed_on_both_sides() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes geo");
    client_a
        .tools_call("host.tool_share", json!({"name": "geo", "visibility": "public"}))
        .await
        .expect("A shares geo publicly");

    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.geo");

    for _ in 0..10 {
        client_b
            .tools_call(&qualified, json!({}))
            .await
            .expect("B calls A's shared tool");
    }

    let usage_a = extract_structured(
        &client_a
            .tools_call("host.usage", json!({}))
            .await
            .expect("A reads usage"),
    );
    assert_eq!(
        usage_a["calls_by_others"][&ns_b], 10,
        "A's calls_by_others must show {ns_b}: 10, got {usage_a}"
    );

    let usage_b = extract_structured(
        &client_b
            .tools_call("host.usage", json!({}))
            .await
            .expect("B reads usage"),
    );
    assert_eq!(
        usage_b["calls_to_shared"], 10,
        "B's calls_to_shared must be 10, got {usage_b}"
    );

    // B's own calls_per_day counter (the quota check's own read) also
    // advanced by 10, even though every row is stored under A's tenant_id.
    let tenant_b = server
        .state
        .db
        .find_tenant_by_namespace(ns_b)
        .await
        .expect("find B")
        .expect("B exists");
    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let used = server
        .state
        .db
        .count_calls_since(tenant_b.id, midnight, true)
        .await
        .expect("count_calls_since");
    assert_eq!(used, 10, "B's calls_per_day counter must have advanced by 10");
}
