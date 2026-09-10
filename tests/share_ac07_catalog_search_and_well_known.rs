//! AC7 — Given three public tools, When `host.catalog.search(q="geo")`
//! runs from any tenant, Then the matching tool is returned with owner
//! namespace and description; `GET /.well-known/mcp/catalog.json` lists the
//! same.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn catalog_search_and_well_known_document_agree() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo_lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish geo_lookup");
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "geo_lookup", "visibility": "public", "description": "geo lookup tool"}),
        )
        .await
        .expect("share geo_lookup");

    // A second, unrelated public tool -- proves the search actually
    // filters rather than returning everything.
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "weather", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish weather");
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "weather", "visibility": "public", "description": "weather tool"}),
        )
        .await
        .expect("share weather");

    // Search runs from ANY tenant, including one with no tools of its own.
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let search = extract_structured(
        &client_b
            .tools_call("host.catalog.search", json!({"q": "geo"}))
            .await
            .expect("catalog search"),
    );
    let tools = search["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "search for 'geo' must match exactly geo_lookup: {tools:?}");
    assert_eq!(tools[0]["name"], format!("{ns_a}.geo_lookup"));
    assert_eq!(tools[0]["owner"], ns_a);
    assert_eq!(tools[0]["description"], "geo lookup tool");

    // The well-known document mirrors the same public listing (unfiltered).
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/.well-known/mcp/catalog.json", server.base_url))
        .send()
        .await
        .expect("GET well-known catalog");
    assert_eq!(resp.status(), 200);
    let doc: serde_json::Value = resp.json().await.expect("parse catalog.json");
    let doc_names: Vec<&str> = doc["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(doc_names.contains(&format!("{ns_a}.geo_lookup").as_str()));
    assert!(doc_names.contains(&format!("{ns_a}.weather").as_str()));
}
