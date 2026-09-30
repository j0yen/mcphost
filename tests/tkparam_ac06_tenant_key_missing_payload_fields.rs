//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC6 (P1) — Given the refusal in AC 1, When `data` is inspected, Then it
//! contains `field` equal to `"tenant_key"`, `expected` containing both the
//! substring `signup` and the substring `Authorization: Bearer`, and
//! `example` a string.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn tenant_key_missing_data_names_field_expected_and_example() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {}}),
        )
        .await
        .expect_err("a call with no tenant_key at all must be refused");

    assert_eq!(err.data.get("field").and_then(|v| v.as_str()), Some("tenant_key"));

    let expected = err
        .data
        .get("expected")
        .and_then(|v| v.as_str())
        .expect("data.expected must be a string");
    assert!(expected.contains("signup"), "expected must mention signup: {expected}");
    assert!(
        expected.contains("Authorization: Bearer"),
        "expected must mention Authorization: Bearer: {expected}"
    );

    assert!(
        err.data.get("example").and_then(|v| v.as_str()).is_some(),
        "data.example must be a string: {:?}",
        err.data
    );
}
