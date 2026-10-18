//! PRD-mcphost-scope-registry-generated
//! AC6 (P1) — Given a tenant with two catalog scopes, When
//! `host.oauth.scopes` runs, Then the registry entries come first with
//! `builtin: true` and the catalog entries after with `builtin: false`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::scopes::Scope;
use serde_json::json;

#[tokio::test]
async fn sreg_ac06_registry_entries_precede_catalog_entries() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Scope Listing Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for (name, desc) in [("billing", "Billing ops"), ("reports", "Run reports")] {
        client
            .tools_call("host.oauth.scope_set", json!({"name": name, "description": desc}))
            .await
            .expect("scope_set");
    }
    let out = client.tools_call("host.oauth.scopes", json!({})).await.expect("host.oauth.scopes");
    let scopes = out["structuredContent"]["scopes"].as_array().expect("scopes array");
    assert_eq!(scopes.len(), Scope::ALL.len() + 2);
    for (entry, scope) in scopes.iter().zip(Scope::ALL) {
        assert_eq!(entry["name"], json!(scope.as_str()));
        assert_eq!(entry["builtin"], json!(true));
    }
    let tail = &scopes[Scope::ALL.len()..];
    assert_eq!(tail[0]["name"], json!("billing"));
    assert_eq!(tail[1]["name"], json!("reports"));
    assert!(tail.iter().all(|e| e["builtin"] == json!(false)));
    assert_eq!(tail[0]["description"], json!("Billing ops"));
}
