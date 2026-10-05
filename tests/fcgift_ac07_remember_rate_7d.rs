//! PRD-mcphost-first-call-gift
//! AC7 — Given seven days of tenants where two of five wrote a
//! first-contact note, When `host.usage` is called, Then
//! `first_contact.remember_rate_7d` is 0.4.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn remember_rate_7d_reflects_two_of_five_tenants() {
    let server = TestServer::start().await;

    // Two of five tenants write a first-contact note at signup.
    let mut keys = Vec::new();
    for i in 0..2 {
        let client = McpClient::new(&server.base_url);
        let signup = extract_structured(
            &client
                .tools_call("signup", json!({"name": format!("AC7 Noted {i}"), "remember": "a first note"}))
                .await
                .expect("signup with remember"),
        );
        keys.push(signup["key"].as_str().expect("key").to_string());
    }
    // The other three sign up with no remember at all.
    for i in 0..3 {
        let client = McpClient::new(&server.base_url);
        let signup = extract_structured(
            &client
                .tools_call("signup", json!({"name": format!("AC7 Bare {i}")}))
                .await
                .expect("signup without remember"),
        );
        keys.push(signup["key"].as_str().expect("key").to_string());
    }

    let usage_client = McpClient::with_bearer(&server.base_url, &keys[0]);
    let usage = extract_structured(
        &usage_client
            .tools_call("host.usage", json!({}))
            .await
            .expect("host.usage"),
    );
    assert_eq!(usage["first_contact"]["remember_rate_7d"], json!(0.4), "{usage}");
}
