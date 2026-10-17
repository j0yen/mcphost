//! PRD-mcphost-publish-visibility-and-tags AC4 (P0) -- Given a tool published
//! `unlisted`, When another tenant runs `host.catalog.search` with no query
//! and anyone reads `GET /.well-known/mcp/catalog.json`, Then neither lists
//! it, while `host.catalog.get` and a cross-tenant call by full name succeed.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn pubvis_ac04() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Pubvis AC4 Owner").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let published = extract_structured(
        &client_a
            .tools_call(
                "host.tool_publish",
                json!({
                    "name": "hidden_echo",
                    "kind": "echo",
                    "spec": {"schema": {"type": "object"}},
                    "visibility": "unlisted",
                }),
            )
            .await
            .expect("unlisted publish"),
    );
    assert_eq!(published["visibility"], json!("unlisted"), "{published}");
    // A listed control tool, so an empty listing cannot pass vacuously.
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "listed_echo", "kind": "echo", "spec": {"schema": {"type": "object"}}, "visibility": "public"}),
        )
        .await
        .expect("public publish");

    let full = format!("{ns_a}.hidden_echo");
    let (_ns_b, key_b) = signup(&server.base_url, "Pubvis AC4 Reader").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let search = extract_structured(&client_b.tools_call("host.catalog.search", json!({})).await.expect("search"));
    let listed: Vec<&str> = search["tools"].as_array().expect("tools").iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(listed.contains(&format!("{ns_a}.listed_echo").as_str()), "control tool listed: {search}");
    assert!(!listed.contains(&full.as_str()), "unlisted tool must not be searchable: {search}");

    let doc: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/.well-known/mcp/catalog.json", server.base_url))
        .send()
        .await
        .expect("GET catalog.json")
        .json()
        .await
        .expect("catalog.json body");
    let doc_names: Vec<&str> = doc["tools"].as_array().expect("tools").iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(doc_names.contains(&format!("{ns_a}.listed_echo").as_str()), "{doc}");
    assert!(!doc_names.contains(&full.as_str()), "unlisted tool must not be in catalog.json: {doc}");

    let got = extract_structured(
        &client_b.tools_call("host.catalog.get", json!({"full_name": full})).await.expect("catalog.get"),
    );
    assert_eq!(got["name"], json!(full), "{got}");
    assert_eq!(got["visibility"], json!("unlisted"), "{got}");

    let call = extract_structured(&client_b.tools_call(&full, json!({"msg": "hi"})).await.expect("cross-tenant call"));
    assert_eq!(call, json!({"msg": "hi"}));
}
