//! AC5 -- PRD-mcphost-resource-listing-shape-in-contract. Given a doc with a
//! human name, When it is listed, Then the contract's
//! `resources.title_field.doc` is `title` and the entry's `title` is the doc
//! name, while table has no `title_field` entry and no listed `title`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::api_contract::COMMITTED_CONTRACT;
use serde_json::{Value, json};

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac05_doc_lists_title_table_does_not() {
    let contract: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    let title_field = &contract["resources"]["title_field"];
    assert_eq!(title_field["doc"], "title");
    assert!(title_field.get("table").is_none(), "table has no title_field: {title_field}");

    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "rlsic ac05").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create table");
    client
        .tools_call("host.docs.put", json!({"name": "notes.md", "content": "# notes"}))
        .await
        .expect("put doc");
    let listed = client.rpc("resources/list", json!({})).await;
    let entries = listed["result"]["resources"].as_array().unwrap_or_else(|| panic!("no resources: {listed}"));
    let by_prefix = |p: &str| entries.iter().find(|e| e["uri"].as_str().is_some_and(|u| u.starts_with(p))).unwrap_or_else(|| panic!("{p} not listed"));
    assert_eq!(by_prefix("mcphost://doc/")[title_field["doc"].as_str().unwrap()], "notes.md");
    assert!(by_prefix("mcphost://table/").get("title").is_none(), "table entries carry no title");
}
