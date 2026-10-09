//! PRD-mcphost-chain-prev-contract AC6 (P0) -- Given a live
//! `host.tool_call` that still hits `resolved to nothing`, When the error
//! is read, Then it carries `available: [<top-level keys of
//! $.prev.result>]` and, when a key matches the missing segment,
//! `did_you_mean`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

async fn call_with_mapping(label: &str, mapping: &str) -> common::RpcError {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let obj = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": obj})).await;
    publish(&client, "step2", "echo", json!({"schema": obj})).await;
    let spec = json!({
        "steps": [
            {"tool": "step1", "args": {"leads": [1], "action": "create"}},
            {"tool": "step2", "args": {"v": mapping}},
        ]
    });
    let chain = publish(&client, "pipeline", "chain", spec).await;
    client
        .tools_call(&chain, json!({}))
        .await
        .expect_err("a mapping that resolves to nothing must end the run")
}

#[tokio::test]
async fn a_typo_d_field_lists_available_keys_and_the_matching_one() {
    let err = call_with_mapping("Chainprev AC6 Typo Tenant", "$.prev.result.lead").await;
    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_missing"));
    assert_eq!(err.data["available"], json!(["action", "leads"]), "{err:?}");
    assert_eq!(err.data["did_you_mean"], json!("$.prev.result.leads"), "{err:?}");
    assert!(err.message.contains("available"), "{err:?}");
}

#[tokio::test]
async fn a_missing_hop_names_the_corrected_path() {
    let err = call_with_mapping("Chainprev AC6 Hop Tenant", "$.prev.action").await;
    assert_eq!(err.data["available"], json!(["action", "leads"]), "{err:?}");
    assert_eq!(err.data["did_you_mean"], json!("$.prev.result.action"), "{err:?}");
}

#[tokio::test]
async fn no_matching_key_means_no_did_you_mean() {
    let err = call_with_mapping("Chainprev AC6 None Tenant", "$.prev.result.zzzzzz").await;
    assert_eq!(err.data["available"], json!(["action", "leads"]), "{err:?}");
    assert!(err.data.get("did_you_mean").is_none(), "{err:?}");
}
