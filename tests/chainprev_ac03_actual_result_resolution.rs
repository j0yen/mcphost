//! PRD-mcphost-chain-prev-contract AC3 (P0) -- Given a predecessor with no
//! declared `outputs` returning `{"leads": [...]}`, When a chain maps
//! `$.prev.result.lead` and is dry-run, Then `will_fail` with `available:
//! ["leads"]` and `did_you_mean: "$.prev.result.leads"`, not `unverifiable`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

async fn dry_run(label: &str, mapping: &str) -> serde_json::Value {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step2", "echo", json!({"schema": schema})).await;
    let spec = json!({
        "steps": [
            {"tool": "step1", "args": {"leads": [{"name": "a"}, {"name": "b"}]}},
            {"tool": "step2", "args": {"lead": mapping}},
        ]
    });
    publish(&client, "pipeline", "chain", spec).await;
    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    common::extract_structured(&result)
}

#[tokio::test]
async fn a_field_the_predecessors_actual_result_lacks_is_will_fail_with_available() {
    let structured = dry_run("Chainprev AC3 Typo Tenant", "$.prev.result.lead").await;
    assert_eq!(structured["verdict"], json!("will_fail"), "{structured}");
    let entry = &structured["evidence"][0];
    assert_eq!(entry["step"], json!(2), "{structured}");
    assert_eq!(entry["path"], json!("$.prev.result.lead"), "{structured}");
    assert_eq!(entry["available"], json!(["leads"]), "{structured}");
    assert_eq!(entry["did_you_mean"], json!("$.prev.result.leads"), "{structured}");
}

#[tokio::test]
async fn a_field_the_predecessors_actual_result_carries_resolves_to_pass() {
    let structured = dry_run("Chainprev AC3 Pass Tenant", "$.prev.result.leads").await;
    assert_eq!(structured["verdict"], json!("pass"), "{structured}");
    assert_eq!(
        structured["steps"][1]["resolved_args"]["lead"],
        json!([{"name": "a"}, {"name": "b"}]),
        "{structured}"
    );
}
