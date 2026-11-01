//! PRD-mcphost-resource-listing-shape-in-contract
//! AC4 (P0) — Given `name_field`, When a table and a doc are listed through
//! `resources/list`, Then each listed `name` equals its kind's `name_field`
//! with the placeholder replaced by the value the listing passed to
//! `uri_for` (`table:{name}` → `table:<name>`, `doc:{id}` → `doc:<ulid>`),
//! and the `uri` equals `uri_scheme` for that kind with the same
//! substitution.

use crate::common::{McpClient, TestServer, signup};
use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

fn listed<'a>(reply: &'a Value, prefix: &str) -> &'a Value {
    reply["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("no resources array: {reply}"))
        .iter()
        .find(|r| r["uri"].as_str().is_some_and(|u| u.starts_with(prefix)))
        .unwrap_or_else(|| panic!("no {prefix} entry: {reply}"))
}

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac04_listed_table_and_doc_follow_name_field_and_uri_scheme() {
    let contract = dump_contract(&KindRegistry::with_builtin());
    let resources = &contract["resources"];
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "rlsic ac04").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create table");
    let put = client
        .tools_call("host.docs.put", json!({"name": "notes.md", "content": "# notes"}))
        .await
        .expect("put doc");
    let doc_id = put["structuredContent"]["id"].as_str().unwrap_or_else(|| panic!("docs.put returned no id: {put}")).to_string();

    let reply = client.rpc("resources/list", json!({})).await;

    let table = listed(&reply, "mcphost://table/");
    let want_name = resources["name_field"]["table"].as_str().unwrap().replace("{name}", "orders");
    assert_eq!(table["name"], want_name.as_str(), "{table}");
    assert_eq!(table["name"], "table:orders", "{table}");
    let want_uri = resources["uri_scheme"]["table"].as_str().unwrap().replace("{name}", "orders");
    assert_eq!(table["uri"], want_uri.as_str(), "{table}");

    let doc = listed(&reply, "mcphost://doc/");
    let want_name = resources["name_field"]["doc"].as_str().unwrap().replace("{id}", &doc_id);
    assert_eq!(doc["name"], want_name.as_str(), "{doc}");
    assert_eq!(doc["name"], format!("doc:{doc_id}").as_str(), "{doc}");
    let want_uri = resources["uri_scheme"]["doc"].as_str().unwrap().replace("{id}", &doc_id);
    assert_eq!(doc["uri"], want_uri.as_str(), "{doc}");
}
