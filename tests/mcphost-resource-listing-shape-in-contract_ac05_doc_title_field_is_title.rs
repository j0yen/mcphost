//! PRD-mcphost-resource-listing-shape-in-contract
//! AC5 (P2) — Given a doc resource with a human name, When it is listed,
//! Then the contract's `resources.title_field.doc` is `title` and the listed
//! entry's `title` equals the doc name, while table has no `title_field`
//! entry and no `title`.

use crate::common::{McpClient, TestServer, signup};
use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

fn listed<'a>(reply: &'a Value, prefix: &str) -> &'a Value {
    reply["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("no resources array: {reply}"))
        .iter()
        .find(|r| r["uri"].as_str().is_some_and(|u| u.starts_with(prefix)))
        .unwrap_or_else(|| panic!("no {prefix} entry: {reply}"))
}

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac05_doc_lists_its_name_in_title_and_table_has_no_title() {
    let committed: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    for (label, contract) in [("committed", committed), ("dumped", dump_contract(&KindRegistry::with_builtin()))] {
        let titles = &contract["resources"]["title_field"];
        assert_eq!(titles["doc"], "title", "{label}: {titles}");
        for kind in ["table", "run", "tool"] {
            assert!(titles.get(kind).is_none(), "{label}: {kind} has no title_field entry: {titles}");
        }
    }

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
    let reply = client.rpc("resources/list", json!({})).await;

    let doc = listed(&reply, "mcphost://doc/");
    assert_eq!(doc["title"], "notes.md", "the human doc name is the title: {doc}");
    assert_ne!(doc["name"], "notes.md", "name carries the id, not the human name: {doc}");
    let table = listed(&reply, "mcphost://table/");
    assert!(table.get("title").is_none(), "a table is listed without a title: {table}");
}
