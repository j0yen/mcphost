//! PRD-mcphost-output-envelope-fields-generated AC7 -- Given an HTTP server
//! with no tenant key, When `GET /contract.json` is requested, Then the body's
//! `output_envelope_fields` equals the dump's, and with the server's tool
//! registry empty the array is unchanged.

use crate::common::TestServer;
use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use serde_json::Value;

async fn served_fields(server: &TestServer) -> Value {
    let resp = reqwest::Client::new().get(format!("{}/contract.json", server.base_url)).send().await.expect("GET /contract.json");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("json body");
    body["output_envelope_fields"].clone()
}

#[tokio::test]
async fn mcphost_output_envelope_fields_generated_ac07() {
    let dump = dump_contract(&KindRegistry::with_builtin())["output_envelope_fields"].clone();
    assert!(dump.as_array().is_some_and(|a| a.len() >= 5), "the dump carries the set: {dump}");

    let server = TestServer::start().await;
    assert_eq!(served_fields(&server).await, dump, "served set equals the dump's");

    let empty = TestServer::start_with_kinds(KindRegistry::new()).await;
    assert_eq!(served_fields(&empty).await, dump, "an empty tool registry leaves the set unchanged");
}
