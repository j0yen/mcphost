//! PRD-mcphost-tool-test-truth AC6 (P1) — Given `host.tool_publish
//! dry_run=true` with the failing chain of AC1, When it runs, Then the
//! gates array carries `verdict: will_fail` with the same failure and the
//! tool is not published; without `dry_run`, Then the tool is published
//! and the response carries the advisory verdict.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

fn failing_chain_spec() -> serde_json::Value {
    json!({
        "steps": [
            {"tool": "step1", "args": {}},
            {"tool": "step2", "args": {"rows": "$.prev.rows"}},
        ]
    })
}

#[tokio::test]
async fn dry_run_gate_reports_will_fail_and_publishes_nothing() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC6 Dry Run Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let step1_spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
        "outputs": ["result", "count"],
    });
    publish(&client, "step1", "http", step1_spec).await;
    publish(&client, "step2", "echo", json!({"schema": {"type": "object"}})).await;

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "pipeline",
                "kind": "chain",
                "spec": failing_chain_spec(),
                "dry_run": true,
            }),
        )
        .await
        .expect("host.tool_publish dry_run=true must succeed");
    let structured = common::extract_structured(&result);

    let gates = structured["gates"].as_array().expect("gates array");
    let verdict_gate = gates
        .iter()
        .find(|g| g["gate"] == json!("verdict"))
        .unwrap_or_else(|| panic!("gates must carry a verdict entry: {structured}"));
    assert_eq!(verdict_gate["verdict"], json!("will_fail"), "{structured}");
    assert_eq!(
        verdict_gate["failures"],
        json!([{
            "step": 2,
            "path": "$.prev.rows",
            "predecessor_keys": ["result", "count"],
            "available": ["result", "count"],
            "did_you_mean": "$.prev.result.rows",
        }]),
        "{structured}"
    );
    assert_eq!(
        verdict_gate["ok"],
        json!(true),
        "the verdict gate is advisory -- it must never fail the overall gate: {structured}"
    );

    let test_result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await;
    assert!(
        test_result.is_err(),
        "a dry_run publish must store nothing -- host.tool_test on it must fail tool_not_found"
    );
}

#[tokio::test]
async fn real_publish_lands_and_carries_the_advisory_verdict() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Tool Test Truth AC6 Real Publish Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let step1_spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
        "outputs": ["result", "count"],
    });
    publish(&client, "step1", "http", step1_spec).await;
    publish(&client, "step2", "echo", json!({"schema": {"type": "object"}})).await;

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pipeline", "kind": "chain", "spec": failing_chain_spec()}),
        )
        .await
        .expect("a will_fail chain must still publish -- advisory only");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["name"], json!(format!("{ns}.pipeline")), "{structured}");
    assert_eq!(structured["verdict"], json!("will_fail"), "{structured}");
    assert_eq!(
        structured["failures"],
        json!([{
            "step": 2,
            "path": "$.prev.rows",
            "predecessor_keys": ["result", "count"],
            "available": ["result", "count"],
            "did_you_mean": "$.prev.result.rows",
        }]),
        "{structured}"
    );

    // Really published: a second publish of the same name must be a
    // republish (no tools_max/count error), and host.tool_test resolves it.
    client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must find the published tool");
}
