//! PRD-mcphost-row-policy
//! AC10 (P0) -- Given an existing rule `region in (EU)` for a table, When
//! `host.policy.set` proposes `region in (EU, US)` without `replace`, Then
//! it is refused naming the widening; `replace: true` confirms it.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn widening_rule_is_refused_unless_replace_is_set() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "integer", "region": "text"}, "primary_key": "id"}),
        )
        .await
        .expect("table create ok");

    client
        .tools_call(
            "host.policy.set",
            json!({
                "target": {"table": "orders"},
                "rule": [{"column_or_attr": "region", "op": "in", "value": {"literal": ["EU"]}}],
            }),
        )
        .await
        .expect("initial policy set ok");

    let refused = client
        .tools_call(
            "host.policy.set",
            json!({
                "target": {"table": "orders"},
                "rule": [{"column_or_attr": "region", "op": "in", "value": {"literal": ["EU", "US"]}}],
            }),
        )
        .await
        .expect_err("widening region from (EU) to (EU, US) must be refused without replace");
    assert_eq!(refused.error_code.as_deref(), Some("policy_widening"), "{refused:?}");
    assert!(refused.message.contains("region"), "message should name the widened column: {refused:?}");

    // A narrower or disjoint rule on the same column is not a widening, so
    // it is accepted without `replace`.
    let narrowed = client
        .tools_call(
            "host.policy.set",
            json!({
                "target": {"table": "orders"},
                "rule": [{"column_or_attr": "region", "op": "in", "value": {"literal": ["EU"]}}],
            }),
        )
        .await
        .expect("re-asserting the same set is not a widening");
    assert_eq!(extract_structured(&narrowed)["version"], json!(2), "{narrowed:?}");

    let confirmed = client
        .tools_call(
            "host.policy.set",
            json!({
                "target": {"table": "orders"},
                "rule": [{"column_or_attr": "region", "op": "in", "value": {"literal": ["EU", "US"]}}],
                "replace": true,
            }),
        )
        .await
        .expect("replace: true confirms the widening");
    assert_eq!(extract_structured(&confirmed)["version"], json!(3), "{confirmed:?}");
}
