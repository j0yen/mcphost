//! PRD-mcphost-tool-scopes-and-consent
//! AC5 (P0) — Given a publish with `scopes: ["admin"]` not in the catalog,
//! or nine scopes, or a name `Bad Scope`, When it runs, Then each is
//! rejected with `invalid_params` naming the rule.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

async fn publish_err(client: &McpClient, scopes: serde_json::Value) -> common::RpcError {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": scopes}),
        )
        .await
        .expect_err("publish must be rejected")
}

#[tokio::test]
async fn scopes_not_in_catalog_too_many_or_badly_named_are_all_invalid_params() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Scope Validation Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("scope_set read");
    client
        .tools_call(
            "host.oauth.scope_set",
            json!({"name": "write", "description": "Change records"}),
        )
        .await
        .expect("scope_set write");

    // Rule 1: a scope not in the catalog and not a built-in.
    let err = publish_err(&client, json!(["admin"])).await;
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
    assert!(
        err.message.contains("catalog") || err.message.contains("admin"),
        "must name the catalog rule: {}",
        err.message
    );

    // Rule 2: at most 8 scopes per tool.
    let nine = json!(["read", "write", "s1", "s2", "s3", "s4", "s5", "s6", "s7"]);
    let err = publish_err(&client, nine).await;
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
    assert!(
        err.message.to_lowercase().contains("8") || err.message.to_lowercase().contains("maximum"),
        "must name the count rule: {}",
        err.message
    );

    // Rule 3: a scope name must match ^[a-z][a-z0-9_:.-]{0,40}$.
    let err = publish_err(&client, json!(["Bad Scope"])).await;
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
    assert!(
        err.message.contains("Bad Scope") || err.message.to_lowercase().contains("match"),
        "must name the naming rule: {}",
        err.message
    );

    // No half-published tool from any of the three rejected attempts.
    let listed = client.tools_call("host.tool_list", json!({})).await.expect("host.tool_list");
    let listed = common::extract_structured(&listed);
    assert!(
        listed["tools"].as_array().unwrap().is_empty(),
        "no rejected publish may have partially landed: {listed:?}"
    );
}
