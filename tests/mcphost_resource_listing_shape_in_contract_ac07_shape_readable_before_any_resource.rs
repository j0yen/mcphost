//! AC7 -- PRD-mcphost-resource-listing-shape-in-contract. Given a freshly
//! signed-up tenant with no tables and no docs, When `resources/list` is
//! called with its key and `GET /contract.json` with none, Then the listing
//! is empty and the contract still carries every `resources` kind.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::resources::RESOURCE_TEMPLATES;
use serde_json::{Value, json};

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac07_empty_tenant_still_reads_the_shape() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "rlsic ac07").await;
    let listed = McpClient::with_bearer(&server.base_url, &key).rpc("resources/list", json!({})).await;
    let entries = listed["result"]["resources"].as_array().unwrap_or_else(|| panic!("no resources array: {listed}"));
    assert!(entries.is_empty(), "a fresh tenant lists nothing: {listed}");

    let body: Value = reqwest::get(format!("{}/contract.json", server.base_url))
        .await
        .expect("GET /contract.json")
        .json()
        .await
        .expect("json body");
    for t in RESOURCE_TEMPLATES {
        let k = t.kind.contract_key();
        assert!(body["resources"]["uri_scheme"][k].is_string(), "uri_scheme.{k}");
        assert!(body["resources"]["name_field"][k].is_string(), "name_field.{k}");
    }
}
