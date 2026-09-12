//! PRD-mcphost-session-key
//! AC4 — Given an anonymous client, When it calls `signup` with a name,
//! Then the result carries `tenant`, `key`, `namespace`, `endpoint`, and a
//! `usage` string that names `tenant_key` as the argument to pass on
//! subsequent calls.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn signup_result_names_tenant_key_in_its_usage_string() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call("signup", json!({"name": "Fresh Agent"}))
        .await
        .expect("signup");
    let structured = extract_structured(&result);

    for field in ["tenant", "key", "namespace", "endpoint", "usage"] {
        assert!(
            structured.get(field).is_some(),
            "signup result must carry {field}: {structured}"
        );
    }
    let usage = structured["usage"].as_str().expect("usage is a string");
    assert!(
        usage.contains("tenant_key"),
        "usage must name tenant_key as the argument to pass: {usage}"
    );
}
