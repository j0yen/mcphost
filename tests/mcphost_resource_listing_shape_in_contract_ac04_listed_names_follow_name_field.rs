//! AC4 -- PRD-mcphost-resource-listing-shape-in-contract. Given the
//! contract's `name_field` and `uri_scheme`, When a table and a doc are
//! listed through `resources/list`, Then each listed `name` and `uri` is
//! the contract string with the placeholder replaced by the value the
//! listing passed to `uri_for`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::api_contract::COMMITTED_CONTRACT;
use serde_json::{Value, json};

fn fill(template: &str, value: &str) -> String {
    let open = template.find('{').expect("placeholder");
    let close = template.find('}').expect("placeholder");
    format!("{}{value}{}", &template[..open], &template[close + 1..])
}

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac04_names_and_uris_follow_the_contract() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "rlsic ac04").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create table");
    client
        .tools_call("host.docs.put", json!({"name": "notes.md", "content": "# notes"}))
        .await
        .expect("put doc");

    let contract: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    let r = &contract["resources"];
    let listed = client.rpc("resources/list", json!({})).await;
    let entries = listed["result"]["resources"].as_array().unwrap_or_else(|| panic!("no resources: {listed}"));

    let table = entries.iter().find(|e| e["uri"].as_str().is_some_and(|u| u.starts_with("mcphost://table/"))).expect("table listed");
    assert_eq!(table["name"], fill(r["name_field"]["table"].as_str().unwrap(), "orders").as_str());
    assert_eq!(table["uri"], fill(r["uri_scheme"]["table"].as_str().unwrap(), "orders").as_str());

    let doc = entries.iter().find(|e| e["uri"].as_str().is_some_and(|u| u.starts_with("mcphost://doc/"))).expect("doc listed");
    let id = doc["uri"].as_str().unwrap().strip_prefix("mcphost://doc/").unwrap();
    assert!(!id.is_empty() && id != "notes.md", "the doc is keyed by its id, not its name: {id}");
    assert_eq!(doc["name"], fill(r["name_field"]["doc"].as_str().unwrap(), id).as_str());
    assert_eq!(doc["uri"], fill(r["uri_scheme"]["doc"].as_str().unwrap(), id).as_str());
}
