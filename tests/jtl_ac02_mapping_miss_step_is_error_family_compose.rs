//! PRD-mcphost-job-timeline-tool AC2 (P0) -- Given a chain whose step 2 fails
//! with `compose_mapping_missing` under `on_error: continue`, When the
//! timeline is read, Then `steps[1]` has `status: error`, `error_class:
//! compose_mapping_missing`, `error_family: compose`, `steps[2]` is `done`,
//! and `totals.error` is 1.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn jtl_ac02() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "JTL2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    for name in ["step1", "step2", "step3"] {
        publish(&client, name, "echo", json!({"schema": schema})).await;
    }
    let chain = publish(
        &client,
        "pipeline",
        "chain",
        json!({"steps": [
            {"tool": "step1", "args": {"n": "$.input.n"}},
            {"tool": "step2", "args": {"m": "$.prev.result.absent"}, "on_error": "continue"},
            {"tool": "step3", "args": {"k": "$.input.n"}}
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({"n": 5})).await.expect("chain call tolerates the miss");

    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let run_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();
    let timeline = extract_structured(
        &client.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect("timeline"),
    );

    let steps = timeline["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "{timeline}");
    assert_eq!(steps[0]["status"], json!("done"), "{timeline}");
    assert_eq!(steps[1]["step"], json!(2), "{timeline}");
    assert_eq!(steps[1]["status"], json!("error"), "{timeline}");
    assert_eq!(steps[1]["error_class"], json!("compose_mapping_missing"), "{timeline}");
    assert_eq!(steps[1]["error_family"], json!("compose"), "{timeline}");
    assert_eq!(steps[2]["status"], json!("done"), "{timeline}");
    assert_eq!(timeline["totals"]["error"], json!(1), "{timeline}");
}
