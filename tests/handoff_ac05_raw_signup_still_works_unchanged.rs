//! PRD-mcphost-handoff-token
//! AC5 (P0) — Given signup without the handoff argument, When it
//! completes, Then the response is byte-compatible with today's raw-key
//! response and all tenant tools work unchanged with that key.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn signup_without_handoff_argument_is_unchanged() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "Plain Tenant"}))
        .await
        .expect("plain signup");
    let result = extract_structured(&raw);

    let fields: BTreeSet<String> = result
        .as_object()
        .expect("signup result is an object")
        .keys()
        .cloned()
        .collect();
    let expected: BTreeSet<String> = ["tenant", "key", "namespace", "endpoint", "usage", "next"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(fields, expected, "raw signup's field set must be unchanged: {result}");
    assert!(result.get("handoff_token").is_none());
    assert!(result["key"].as_str().is_some_and(|k| !k.is_empty()));
    assert_eq!(result["next"].as_str(), Some("host.quickstart"));
}

#[tokio::test]
async fn signup_with_explicit_handoff_false_is_also_unchanged() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "Plain Tenant Explicit", "handoff": false}))
        .await
        .expect("signup with handoff: false");
    let result = extract_structured(&raw);
    assert!(result.get("key").is_some());
    assert!(result.get("handoff_token").is_none());
}

#[tokio::test]
async fn raw_key_still_works_across_representative_tenant_tools() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let raw = client
        .tools_call("signup", json!({"name": "Raw Flow Tenant"}))
        .await
        .expect("plain signup");
    let result = extract_structured(&raw);
    let key = result["key"].as_str().expect("key").to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami with the raw key");
    tenant_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("host.tool_publish with the raw key");
    tenant_client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("host.tool_list with the raw key");
    tenant_client
        .tools_call("host.usage", json!({}))
        .await
        .expect("host.usage with the raw key");
}
