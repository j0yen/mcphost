//! PRD-mcphost-chain-prev-contract AC5 (P0) -- Given any chain dry run,
//! When `resolved_args` contain an `unresolved_path`, Then the top-level
//! verdict is not `pass` and `success` is not `true`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::{Value, json};

fn has_unresolved_path(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.contains_key("unresolved_path") || m.values().any(has_unresolved_path),
        Value::Array(a) => a.iter().any(has_unresolved_path),
        _ => false,
    }
}

#[tokio::test]
async fn no_dry_run_with_an_unresolved_path_reports_pass() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chainprev AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let obj = json!({"type": "object"});
    publish(&client, "src", "echo", json!({"schema": obj})).await;
    publish(&client, "sink", "echo", json!({"schema": obj})).await;
    publish(
        &client,
        "fetcher",
        "http",
        json!({"method": "GET", "url": "http://127.0.0.1:1/x", "args_schema": obj}),
    )
    .await;

    let chains = [
        // a typo'd field on a predecessor that ran
        ("typo", json!([{"tool": "src", "args": {"a": 1}}, {"tool": "sink", "args": {"x": "$.prev.result.b"}}])),
        // a missing hop
        ("hop", json!([{"tool": "src", "args": {"a": 1}}, {"tool": "sink", "args": {"x": "$.prev.a"}}])),
        // a predecessor that cannot run
        ("down", json!([{"tool": "fetcher", "args": {}}, {"tool": "sink", "args": {"x": "$.prev.result.a"}}])),
        // an input the dry run was not given
        ("input", json!([{"tool": "src", "args": {"a": "$.input.missing"}}, {"tool": "sink", "args": {}}])),
        // a `$.prev` on the very first step
        ("first", json!([{"tool": "sink", "args": {"x": "$.prev.result.a"}}])),
    ];
    let mut saw_unresolved = 0;
    for (name, steps) in chains {
        let chain = format!("chain_{name}");
        publish(&client, &chain, "chain", json!({"steps": steps})).await;
        let result = client
            .tools_call("host.tool_test", json!({"name": chain, "args": {}}))
            .await
            .expect("host.tool_test must succeed");
        let structured = common::extract_structured(&result);
        if has_unresolved_path(&structured["steps"]) {
            saw_unresolved += 1;
            assert_ne!(structured["verdict"], json!("pass"), "{name}: {structured}");
            assert_ne!(structured["success"], json!(true), "{name}: {structured}");
        }
    }
    assert!(saw_unresolved >= 3, "the scenarios must actually produce unresolved paths");
}
