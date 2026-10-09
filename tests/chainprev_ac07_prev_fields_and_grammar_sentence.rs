//! PRD-mcphost-chain-prev-contract AC7 (P1) -- Given a chain publish
//! response, When read, Then each step after the first lists `prev_fields`
//! from the predecessor's declared `outputs` or its dry-run result, and
//! `docs/kinds/chain.md` states the derived grammar sentence that a test
//! checks names every accepted key.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use mcphost::kinds::chain::{accepted_prev_keys, accepted_step_keys, grammar_sentence};
use serde_json::json;

#[tokio::test]
async fn publish_lists_prev_fields_from_declared_outputs() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chainprev AC7 Publish Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(
        &client,
        "declared",
        "http",
        json!({"method": "GET", "url": "http://127.0.0.1:1/x", "args_schema": {"type": "object"},
               "outputs": ["rows", "count"]}),
    )
    .await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({"steps": [
        {"tool": "declared", "args": {}},
        {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
    ]});
    let result = client
        .tools_call("host.tool_publish", json!({"name": "pipeline", "kind": "chain", "spec": spec}))
        .await
        .expect("publish");
    let structured = common::extract_structured(&result);
    let steps = structured["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "only steps after the first: {structured}");
    assert_eq!(steps[0]["step"], json!(2));
    assert_eq!(steps[0]["prev_fields"], json!(["rows", "count"]), "{structured}");
}

#[tokio::test]
async fn dry_run_lists_prev_fields_from_the_predecessors_result() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chainprev AC7 Dry Run Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({"steps": [
        {"tool": "src", "args": {"leads": [1], "owner": "x"}},
        {"tool": "sink", "args": {"leads": "$.prev.result.leads"}},
    ]});
    let publish_result = client
        .tools_call("host.tool_publish", json!({"name": "pipeline", "kind": "chain", "spec": spec}))
        .await
        .expect("publish");
    let published = common::extract_structured(&publish_result);
    assert_eq!(published["steps"][0]["prev_fields"], json!(null), "{published}");

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("tool_test");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["steps"][1]["prev_fields"], json!(["leads", "owner"]), "{structured}");
}

#[test]
fn chain_doc_states_the_derived_grammar_sentence_naming_every_accepted_key() {
    let sentence = grammar_sentence();
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/kinds/chain.md"))
        .expect("docs/kinds/chain.md");
    assert!(doc.contains(&sentence), "chain.md must state: {sentence}");
    for key in accepted_prev_keys().into_iter().chain(accepted_step_keys()) {
        assert!(sentence.contains(&format!("`{key}`")), "sentence must name `{key}`: {sentence}");
    }
    assert_eq!(
        sentence,
        "`$.prev` has exactly one key, `result`; `$.steps[i]` has exactly one key, `result`."
    );
}
