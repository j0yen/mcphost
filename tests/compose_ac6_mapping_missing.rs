//! PRD-mcphost-composition AC6 (P0) — Given a chain whose second step's
//! mapping path resolves to nothing, When called, Then the run ends
//! `error` with `compose_mapping_missing` naming the path and
//! `failed_step: 2` (step one's own completion isn't separately checkable
//! here -- no `runs` table yet, see `kinds::chain`'s module doc; AC5
//! already covers a step's own successful dispatch).

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_step_two_mapping_miss_ends_the_run_with_compose_mapping_missing() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Mapping Missing Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step2", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "step1", "args": {"n": "$.input.n"}},
            // step1's own echo result is `{"n": <n>}`; ".missing" is never
            // there.
            {"tool": "step2", "args": {"m": "$.prev.result.missing"}}
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let err = client
        .tools_call(&chain, json!({"n": 5}))
        .await
        .expect_err("a mapping that resolves to nothing must end the run");
    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_missing"));
    assert_eq!(err.data["path"], json!("$.prev.result.missing"));
    assert_eq!(err.data["failed_step"], json!(2));
    assert_eq!(err.data["step_tool"], json!("step2"));
}
