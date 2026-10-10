//! AC5 — Given three refusals and one success in the window, When
//! `host.usage` is called, Then it reports `refused.total = 3` with
//! `by_code` counts and `calls = 1`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn host_usage_reports_refused_total_and_by_code() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Refused Usage Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "intool",
                "kind": "echo",
                "spec": {"schema": {
                    "type": "object",
                    "properties": {"n": {"type": "integer"}},
                    "required": ["n"]
                }}
            }),
        )
        .await
        .expect("publish");
    let qualified = format!("{ns}.intool");

    client.tools_call(&qualified, json!({"n": 1})).await.expect("one success");
    for bad in [json!({"n": 4.78}), json!({"n": "x"})] {
        let err = client.tools_call(&qualified, bad).await.expect_err("args_invalid");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
    }
    // Third refusal via a pinned version that does not exist (its wire code
    // is also `args_invalid`).
    let err = client
        .tools_call("host.tool_call", json!({"name": "intool", "args": {"n": 1}, "version": 99}))
        .await
        .expect_err("pinned version missing");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));

    let usage = client.tools_call("host.usage", json!({"window": "24h"})).await.expect("host.usage");
    let usage = extract_structured(&usage);
    assert_eq!(usage["calls"].as_i64(), Some(1), "{usage}");
    assert_eq!(usage["refused"]["total"].as_i64(), Some(3), "{usage}");
    assert_eq!(usage["refused"]["by_code"], json!({"args_invalid": 3}), "{usage}");
}
