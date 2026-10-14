//! PRD-mcphost-usage-in-every-response AC2 -- Given the pre-PRD golden
//! result of the echo tool, When the same call runs after this PRD, Then
//! `content` and `structuredContent` are byte-identical and `_meta.usage` is
//! present with the four fields.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn echo_content_is_byte_identical_and_meta_usage_has_four_fields() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Meta Golden").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let result = client.tools_call(&format!("{ns}.hello"), json!({"msg": "hi", "n": 1})).await.expect("call");

    // Captured from the echo tool before this PRD (the result carried no
    // `_meta`): compared as serialized bytes, not as parsed values.
    assert_eq!(serde_json::to_string(&result["content"]).unwrap(), GOLDEN_CONTENT);
    assert_eq!(serde_json::to_string(&result["structuredContent"]).unwrap(), GOLDEN_STRUCTURED);

    let usage = &result["_meta"]["usage"]["calls_per_day"];
    for field in ["used", "limit", "remaining", "resets_at"] {
        assert!(!usage[field].is_null(), "_meta.usage.calls_per_day.{field} missing: {result}");
    }
    assert_eq!(usage["used"], json!(1));
}

const GOLDEN_CONTENT: &str = r#"[{"text":"{\"msg\":\"hi\",\"n\":1}","type":"text"}]"#;
const GOLDEN_STRUCTURED: &str = r#"{"msg":"hi","n":1}"#;
