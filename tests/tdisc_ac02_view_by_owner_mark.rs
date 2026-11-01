//! PRD-mcphost-trusted-discovery-catalog-receipts AC2 (P0) -- a plain or
//! anonymous reader lists only real, enabled owners' public tools; a
//! synthetic-marked reader adds the synthetic owners'; a disabled owner's tool
//! is listed in neither view; `include_synthetic` is in the contract dump.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_plain, signup_with_synthetic_header};
use mcphost::api_contract::{COMMITTED_CONTRACT, dump_contract};
use mcphost::kinds::KindRegistry;
use rusqlite::Connection;
use serde_json::{Value, json};

async fn share_public(client: &McpClient, name: &str, description: &str) {
    client
        .tools_call("host.tool_publish", json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    client
        .tools_call("host.tool_share", json!({"name": name, "visibility": "public", "description": description}))
        .await
        .expect("share");
}

fn names(listing: &Value) -> Vec<String> {
    listing["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect()
}

async fn search(client: &McpClient, args: Value) -> Vec<String> {
    names(&extract_structured(&client.tools_call("host.catalog.search", args).await.expect("search")))
}

async fn shared(client: &McpClient) -> Vec<String> {
    names(&extract_structured(&client.tools_call("host.tool_list_shared", json!({"q": "tdisc ac2"})).await.expect("list_shared")))
}

#[tokio::test]
async fn tdisc_ac02() {
    let server = TestServer::start().await;
    let (plain_ns, plain_key) = signup_plain(&server.base_url, "Tdisc AC2 Plain Owner").await;
    let syn = signup_with_synthetic_header(&server.base_url, "Tdisc AC2 Synthetic Owner", "tdisc:ac2-owner").await;
    let (syn_ns, syn_key) = (syn["tenant"].as_str().unwrap().to_string(), syn["key"].as_str().unwrap().to_string());
    let (dead_ns, dead_key) = signup_plain(&server.base_url, "Tdisc AC2 Disabled Owner").await;

    share_public(&McpClient::with_bearer(&server.base_url, &plain_key), "plain_tool", "tdisc ac2 plain").await;
    share_public(&McpClient::with_bearer(&server.base_url, &syn_key), "syn_tool", "tdisc ac2 synthetic").await;
    share_public(&McpClient::with_bearer(&server.base_url, &dead_key), "dead_tool", "tdisc ac2 disabled").await;
    Connection::open(server.data_dir.0.join("mcphost.db"))
        .unwrap()
        .execute("UPDATE tenants SET disabled = 1, disabled_reason = 'self_offboard' WHERE namespace = ?1", [&dead_ns])
        .unwrap();

    let plain_tool = format!("{plain_ns}.plain_tool");
    let syn_tool = format!("{syn_ns}.syn_tool");
    let dead_tool = format!("{dead_ns}.dead_tool");

    // anonymous well-known document
    let doc: Value = reqwest::get(format!("{}/.well-known/mcp/catalog.json", server.base_url))
        .await
        .expect("GET well-known")
        .json()
        .await
        .expect("json");
    assert_eq!(names(&doc), vec![plain_tool.clone()], "{doc}");

    // a plain reader
    let (_, reader_key) = signup_plain(&server.base_url, "Tdisc AC2 Plain Reader").await;
    let plain = McpClient::with_bearer(&server.base_url, &reader_key);
    assert_eq!(search(&plain, json!({})).await, vec![plain_tool.clone()]);
    assert_eq!(shared(&plain).await, vec![plain_tool.clone()]);

    // another synthetic-marked tenant adds the synthetic owner's tool
    let other = signup_with_synthetic_header(&server.base_url, "Tdisc AC2 Synthetic Reader", "tdisc:ac2-reader").await;
    let synthetic = McpClient::with_bearer(&server.base_url, other["key"].as_str().unwrap());
    let mut got = search(&synthetic, json!({})).await;
    got.sort();
    let mut want = vec![plain_tool.clone(), syn_tool.clone()];
    want.sort();
    assert_eq!(got, want);
    let mut got = shared(&synthetic).await;
    got.sort();
    assert_eq!(got, want);

    // the explicit override, both ways
    let mut got = search(&plain, json!({"include_synthetic": true})).await;
    got.sort();
    assert_eq!(got, want, "include_synthetic true shows the synthetic owner to a plain caller");
    assert_eq!(search(&synthetic, json!({"include_synthetic": false})).await, vec![plain_tool.clone()]);

    // a disabled owner's tool is in no view
    for listing in [search(&plain, json!({"include_synthetic": true})).await, search(&synthetic, json!({})).await, shared(&synthetic).await] {
        assert!(!listing.contains(&dead_tool), "{listing:?}");
    }

    // a direct lookup stays a direct lookup
    plain.tools_call("host.catalog.get", json!({"full_name": syn_tool})).await.expect("catalog.get is not discovery");

    // include_synthetic is in the contract dump
    let live = dump_contract(&KindRegistry::with_builtin());
    let tool = live["tools"].as_array().unwrap().iter().find(|t| t["name"] == "host.catalog.search").expect("tool");
    assert_eq!(tool["input_schema"]["properties"]["include_synthetic"]["type"], json!("boolean"), "{tool}");
    let committed: Value = serde_json::from_str(COMMITTED_CONTRACT).expect("committed contract parses");
    let committed_tool = committed["tools"].as_array().unwrap().iter().find(|t| t["name"] == "host.catalog.search").unwrap();
    assert_eq!(committed_tool["input_schema"], tool["input_schema"], "the committed contract is the dump");
}
