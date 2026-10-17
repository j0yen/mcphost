//! PRD-mcphost-owner-verification-paths AC2 (P0) -- Given the
//! `VERIFICATION_METHODS` const, When the drift test runs, Then every
//! method's tool name resolves to a handler arm, each `host.claim.*`
//! descriptor's `method` enum equals the const, and `/claim/methods.json`
//! equals it.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::verify::{VERIFICATION_METHODS, claim_tool_names};
use serde_json::{Value, json};

fn const_names() -> Vec<&'static str> {
    VERIFICATION_METHODS.iter().map(|(m, _)| *m).collect()
}

#[tokio::test]
async fn ownv_ac02() {
    // Descriptors: every host.claim.* tool carries the const as its `method` enum,
    // and the family is exactly what the table (plus its follow-ups) names.
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let claim: Vec<_> = tools.iter().filter(|t| t.name.starts_with("host.claim.")).collect();
    let mut advertised: Vec<String> = claim.iter().map(|t| t.name.to_string()).collect();
    advertised.sort();
    let mut expected: Vec<String> = claim_tool_names().iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(advertised, expected, "host.claim.* descriptors vs the table");
    for tool in &claim {
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(schema["properties"]["method"]["enum"], json!(const_names()), "{}: {schema}", tool.name);
    }

    // The migration's CHECK list is the const too.
    let migration = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/0120_owner_verification_method.sql"))
        .expect("migration 0120");
    let quoted: Vec<String> = const_names().iter().map(|m| format!("'{m}'")).collect();
    assert!(migration.contains(&format!("owner_method IN ({})", quoted.join(", "))), "migration CHECK vs const");

    // Handler arms: no method's tool is answered `tool_not_found`.
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ownv AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for (method, tool) in VERIFICATION_METHODS {
        if let Err(e) = client.tools_call(tool, json!({"method": method})).await {
            assert_ne!(e.error_code.as_deref(), Some("tool_not_found"), "{tool} has no handler arm: {e:?}");
        }
    }
    if let Err(e) = client.tools_call("host.claim.dns.check", json!({"domain": "acme.dev"})).await {
        assert_ne!(e.error_code.as_deref(), Some("tool_not_found"), "host.claim.dns.check has no handler arm: {e:?}");
    }

    // /claim/methods.json equals the const (name and tool, in order).
    let body: Value = reqwest::get(format!("{}/claim/methods.json", server.base_url))
        .await
        .expect("GET methods.json")
        .json()
        .await
        .expect("json");
    let listed: Vec<(String, String)> = body["methods"]
        .as_array()
        .expect("methods array")
        .iter()
        .map(|m| (m["name"].as_str().unwrap_or_default().to_string(), m["tool"].as_str().unwrap_or_default().to_string()))
        .collect();
    let want: Vec<(String, String)> = VERIFICATION_METHODS.iter().map(|(m, t)| (m.to_string(), t.to_string())).collect();
    assert_eq!(listed, want, "{body}");
}
