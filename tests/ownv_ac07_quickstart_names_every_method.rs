//! PRD-mcphost-owner-verification-paths AC7 (P1) -- Given an unverified
//! tenant, When `host.quickstart` is read, Then `next` names email, github
//! and dns from the generated table.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::verify::VERIFICATION_METHODS;
use serde_json::json;

#[tokio::test]
async fn ownv_ac07() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ownv AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let next = quickstart["next"].as_array().expect("next array");
    let step = next
        .iter()
        .find(|s| s["methods"].is_array())
        .unwrap_or_else(|| panic!("no verification step in next: {quickstart}"));
    let listed: Vec<(String, String)> = step["methods"]
        .as_array()
        .expect("methods")
        .iter()
        .map(|m| (m["method"].as_str().unwrap_or_default().to_string(), m["call"].as_str().unwrap_or_default().to_string()))
        .collect();
    let want: Vec<(String, String)> = VERIFICATION_METHODS.iter().map(|(m, t)| (m.to_string(), t.to_string())).collect();
    assert_eq!(listed, want, "{step}");
    let note = step["note"].as_str().expect("note");
    for name in ["email", "github", "dns"] {
        assert!(note.contains(name), "note names {name}: {note}");
    }
}
