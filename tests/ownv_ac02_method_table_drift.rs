//! PRD-mcphost-owner-verification-paths
//! AC2 (P0) — Given the `VERIFICATION_METHODS` const, When the drift test
//! runs, Then every method's tool name resolves to a handler arm, each
//! `host.claim.*` descriptor's `method` enum equals the const, and
//! `/claim/methods.json` equals it.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::verify::{DNS_CHECK_TOOL, VERIFICATION_METHODS, agent_startable, method_names};
use serde_json::{Value, json};

#[tokio::test]
async fn ownv_ac02_every_method_tool_has_a_handler_arm() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ownv AC2 Arms").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut tools: Vec<&str> = VERIFICATION_METHODS.iter().map(|(_, tool)| *tool).collect();
    tools.push(DNS_CHECK_TOOL);
    for tool in tools {
        let outcome = client.tools_call(tool, json!({"domain": "ownv-ac02.example.dev"})).await;
        if let Err(e) = outcome {
            assert_ne!(
                e.error_code.as_deref(),
                Some("tool_not_found"),
                "{tool} has a descriptor but no handler arm: {}",
                e.message
            );
        }
    }
}

#[tokio::test]
async fn ownv_ac02_descriptor_enums_and_methods_json_equal_the_const() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ownv AC2 Enums").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let want: Vec<Value> = method_names().into_iter().map(Value::from).collect();

    let list = client.tools_list().await.expect("tools/list");
    let claim_tools: Vec<&Value> = list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter(|t| t["name"].as_str().is_some_and(|n| n.starts_with("host.claim.")))
        .collect();
    for (_, tool) in agent_startable() {
        assert!(claim_tools.iter().any(|t| t["name"] == *tool), "{tool} missing from tools/list");
    }
    assert!(claim_tools.iter().any(|t| t["name"] == DNS_CHECK_TOOL), "{DNS_CHECK_TOOL} missing");
    for t in claim_tools {
        assert_eq!(
            t["inputSchema"]["properties"]["method"]["enum"],
            json!(want),
            "{}: method enum drifted from VERIFICATION_METHODS",
            t["name"]
        );
    }

    let doc: Value = reqwest::get(format!("{}/claim/methods.json", server.base_url))
        .await
        .expect("GET /claim/methods.json")
        .json()
        .await
        .expect("json body");
    let names: Vec<&str> = doc["methods"].as_array().expect("methods").iter().map(|m| m["name"].as_str().unwrap()).collect();
    let tools: Vec<&str> = doc["methods"].as_array().unwrap().iter().map(|m| m["tool"].as_str().unwrap()).collect();
    assert_eq!(names, agent_startable().map(|(n, _)| *n).collect::<Vec<_>>());
    assert_eq!(tools, agent_startable().map(|(_, t)| *t).collect::<Vec<_>>());
}

#[test]
fn ownv_ac02_migration_check_equals_the_const() {
    let sql = include_str!("../migrations/0125_owner_method_admin.sql");
    let check = sql
        .lines()
        .find(|l| l.contains("owner_method TEXT CHECK"))
        .expect("owner_method CHECK line");
    let listed = check.split_once("IN (").expect("IN list").1.split(')').next().unwrap();
    let in_migration: Vec<&str> = listed.split(',').map(|s| s.trim().trim_matches('\'')).collect();
    assert_eq!(in_migration, method_names());
}
