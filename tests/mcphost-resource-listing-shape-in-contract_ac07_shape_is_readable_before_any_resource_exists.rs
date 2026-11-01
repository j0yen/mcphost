//! PRD-mcphost-resource-listing-shape-in-contract
//! AC7 (P0) — Given a freshly signed-up test tenant with no tables and no
//! docs, When `resources/list` is called with its key and
//! `GET /contract.json` with no key, Then the listing is empty and the
//! contract still carries all `resources` kinds.

use crate::common::{McpClient, TestServer, signup};
use mcphost::resources::RESOURCE_TEMPLATES;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac07_empty_tenant_lists_nothing_and_contract_still_has_every_kind() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "rlsic ac07").await;

    let listing = McpClient::with_bearer(&server.base_url, &key).rpc("resources/list", json!({})).await;
    let listed = listing["result"]["resources"].as_array().unwrap_or_else(|| panic!("no resources array: {listing}"));
    assert!(listed.is_empty(), "a tenant with no tables, docs, runs or tools lists nothing: {listing}");

    let contract: Value = reqwest::Client::new()
        .get(format!("{}/contract.json", server.base_url))
        .send()
        .await
        .expect("GET /contract.json")
        .json()
        .await
        .expect("json body");
    let kinds: BTreeSet<String> = RESOURCE_TEMPLATES.iter().map(|t| t.kind.key().to_string()).collect();
    assert_eq!(kinds.len(), 4, "table, doc, run, tool");
    for field in ["uri_scheme", "name_field"] {
        let have: BTreeSet<String> = contract["resources"][field]
            .as_object()
            .unwrap_or_else(|| panic!("no resources.{field}: {contract}"))
            .keys()
            .cloned()
            .collect();
        assert_eq!(have, kinds, "resources.{field} carries every kind before any resource exists");
    }
}
