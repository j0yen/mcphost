//! AC3 — Given a spec referencing `secret.missing`, When published, Then it
//! is rejected with `secret_missing` naming the secret.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn unknown_secret_reference_is_rejected_at_publish() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Missing Secret Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": "https://api.example.com/v1/ping",
        "headers": {"Authorization": "Bearer {{secret.missing}}"},
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "needs_secret", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("publishing a spec referencing an unset secret must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("secret_missing"));
    assert!(
        err.message.contains("missing"),
        "error message should name the missing secret, got: {}",
        err.message
    );
}

#[tokio::test]
async fn a_secret_that_is_set_publishes_fine() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Has Secret Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "stripe", "value": "sk_test_1"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "method": "GET",
        "url": "https://api.example.com/v1/ping",
        "headers": {"Authorization": "Bearer {{secret.stripe}}"},
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "has_secret", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publishing a spec referencing a set secret must succeed");
}
