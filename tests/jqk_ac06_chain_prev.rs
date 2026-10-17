//! PRD-mcphost-jq-kind
//! AC6 -- Given a chain `[{tool: fetch_rows}, {tool: failing_rows, args:
//! "$.prev"}]`, When called, Then the jq step sees the first step's result as
//! `.` and `$prev`; `host.tool_test` on the chain reports the resolved
//! `prev_fields`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::chain::ChainKind;
use mcphost::kinds::jq::JqKind;
use serde_json::{Value, json};
use std::sync::Arc;

fn rows() -> Value {
    json!([{"id": 1, "status": "ok"}, {"id": 2, "status": "failed"}, {"id": 3, "status": "late"}])
}

/// The server, a signed-up client, and `failing_rows`' qualified name.
async fn setup() -> (TestServer, McpClient, String) {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ChainKind));
    kinds.register(Arc::new(JqKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "Jqk AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(
        &client,
        "fetch_rows",
        "jq",
        json!({"program": json!({"rows": rows()}).to_string(), "args_schema": {}}),
    )
    .await;
    let failing = publish(
        &client,
        "failing_rows",
        "jq",
        json!({"program": "{seen: ., prev: $prev, failing: [.rows[] | select(.status != \"ok\") | .id]}"}),
    )
    .await;
    (server, client, failing)
}

fn chain_spec() -> Value {
    json!({"steps": [
        {"tool": "fetch_rows"},
        {"tool": "failing_rows", "args": "$.prev"},
    ]})
}

#[tokio::test]
async fn jqk_ac06_the_jq_step_sees_the_previous_result_as_dot_and_prev() {
    let (_server, client, _failing) = setup().await;
    let chain = publish(&client, "pipeline", "chain", chain_spec()).await;
    let result = client.tools_call(&chain, json!({})).await.expect("the chain runs");
    let structured = extract_structured(&result);
    let first = json!({"rows": rows()});
    assert_eq!(structured["result"]["seen"], first, "`.` is the first step's result: {structured}");
    assert_eq!(structured["result"]["prev"], first, "`$prev` is the first step's result: {structured}");
    assert_eq!(structured["result"]["failing"], json!([2, 3]), "{structured}");
}

#[tokio::test]
async fn jqk_ac06_prev_is_null_outside_a_chain() {
    let (_server, client, failing) = setup().await;
    let direct = client
        .tools_call(&failing, json!({"rows": rows()}))
        .await
        .expect("a direct call");
    assert_eq!(extract_structured(&direct)["prev"], Value::Null);
}

#[tokio::test]
async fn jqk_ac06_tool_test_reports_the_resolved_prev_fields() {
    let (_server, client, _failing) = setup().await;
    publish(&client, "pipeline", "chain", chain_spec()).await;
    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("tool_test");
    let structured = extract_structured(&result);
    assert_eq!(structured["steps"][1]["prev_fields"], json!(["rows"]), "{structured}");
    assert_eq!(structured["steps"][1]["resolved_args"], json!({"rows": rows()}), "{structured}");
}

#[tokio::test]
async fn jqk_ac06_a_string_args_other_than_prev_is_refused_at_publish() {
    let (_server, client, _failing) = setup().await;
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad", "kind": "chain", "spec": {"steps": [
                {"tool": "fetch_rows"},
                {"tool": "failing_rows", "args": "$.input"},
            ]}}),
        )
        .await
        .expect_err("only \"$.prev\" is accepted as a string");
    assert!(err.message.contains("$.prev"), "{}", err.message);
}
