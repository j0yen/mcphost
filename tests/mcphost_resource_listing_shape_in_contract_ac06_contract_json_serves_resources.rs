//! AC6 -- PRD-mcphost-resource-listing-shape-in-contract. Given an HTTP
//! server with no tenant key, When `GET /contract.json` is requested, Then
//! the body's `resources` equals the dump's, the ETag is the contract sha,
//! and a conditional request gets 304.

use crate::common;
use common::TestServer;
use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use serde_json::Value;

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac06_served_resources_equal_the_dump() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let url = format!("{}/contract.json", server.base_url);

    let resp = http.get(&url).send().await.expect("GET /contract.json");
    assert_eq!(resp.status(), 200);
    let etag = format!("\"{}\"", server.state.contract.sha);
    assert_eq!(resp.headers()["etag"].to_str().unwrap(), etag);
    let body: Value = resp.json().await.expect("json body");
    let dump = dump_contract(&KindRegistry::with_builtin());
    assert_eq!(body["resources"], dump["resources"]);
    assert_eq!(body["resources"], mcphost::resources::contract_section());

    let cached = http.get(&url).header("If-None-Match", &etag).send().await.expect("conditional GET");
    assert_eq!(cached.status(), 304);
}
