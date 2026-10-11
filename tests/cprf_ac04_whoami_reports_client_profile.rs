//! PRD-mcphost-client-profile-registry AC4 — Given a keyed tenant whose
//! session announced `claude-code`, When it calls `host.whoami`, Then
//! `client_profile.id == "claude_code"`, `from == "clientInfo"`, and the seven
//! limit fields equal the table row.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::client_profiles::{CLIENT_PROFILES, whoami_json};
use serde_json::json;

async fn keyed(server: &TestServer, client_info: Option<(&str, &str)>) -> McpClient {
    let signup = McpClient::new(&server.base_url);
    let result = signup.tools_call("signup", json!({"name": "Profile Caller"})).await.expect("signup");
    let key = extract_structured(&result)["key"].as_str().expect("key").to_string();
    let client = McpClient::with_bearer(&server.base_url, &key);
    match client_info {
        Some((n, v)) => client.with_client_info(n, v),
        None => client,
    }
}

#[tokio::test]
async fn whoami_reports_the_claude_code_row() {
    let server = TestServer::start().await;
    let client = keyed(&server, Some(("claude-code", "2.1"))).await;
    let whoami = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let row = CLIENT_PROFILES.iter().find(|p| p.id == "claude_code").expect("row");
    let profile = &whoami["client_profile"];
    assert_eq!(profile["id"], json!("claude_code"), "{whoami:?}");
    assert_eq!(profile["from"], json!("clientInfo"), "{whoami:?}");
    assert_eq!(
        profile,
        &whoami_json(row, Some(mcphost::client_profiles::From::ClientInfo)),
        "seven limit fields equal the table row"
    );
    for key in ["tool_name_max", "nested_depth_max", "structured_content", "oauth", "result_bytes_max"] {
        assert!(!profile[key].is_null(), "{key} present: {profile:?}");
    }
    assert_eq!(profile["tool_name_max"], json!(row.tool_name_max));
    assert_eq!(profile["oauth"], json!("none"));
}

#[tokio::test]
async fn unrecognised_client_reports_unknown_with_null_from() {
    let server = TestServer::start().await;
    let client = keyed(&server, Some(("acme-agent", "1.0"))).await;
    let whoami = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(whoami["client_profile"]["id"], json!("unknown"), "{whoami:?}");
    assert!(whoami["client_profile"]["from"].is_null(), "{whoami:?}");
}
