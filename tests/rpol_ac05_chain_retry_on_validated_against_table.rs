//! PRD-mcphost-retry-policy-table-generated AC5 (R5) — Given a chain step
//! with `retry.on: ["args_invalid"]`, When published, Then it is refused and
//! `data.accepted` equals the retryable codes of the table.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use mcphost::retry_policy::RETRY_POLICY;
use serde_json::{Value, json};

#[tokio::test]
async fn rpol_ac05() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Rpol Ac05").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "fetch_rows", "echo", json!({"schema": {"type": "object"}})).await;

    let expected: Vec<&str> = RETRY_POLICY.iter().filter(|r| r.retryable).map(|r| r.code).collect();
    assert!(!expected.is_empty());

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "pipeline",
                "kind": "chain",
                "spec": {"steps": [{"tool": "fetch_rows", "args": {}, "retry": {"on": ["args_invalid"]}}]},
            }),
        )
        .await
        .expect_err("retry.on naming a non-retryable code is refused at publish");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    let accepted: Vec<&str> =
        err.data["accepted"].as_array().expect("data.accepted").iter().filter_map(Value::as_str).collect();
    assert_eq!(accepted, expected, "data.accepted is the table's retryable codes");
    assert_eq!(err.data["refused"], json!(["args_invalid"]));

    // Nothing was published, and a retryable code is accepted.
    let ok = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "pipeline",
                "kind": "chain",
                "spec": {"steps": [{"tool": "fetch_rows", "args": {}, "retry": {"on": ["upstream_error", "tool_timeout"]}}]},
            }),
        )
        .await;
    assert!(ok.is_ok(), "retry.on over retryable codes publishes: {ok:?}");
}
