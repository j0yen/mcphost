//! PRD-mcphost-usage-in-every-response AC3 -- Given `host.usage` on a keyed
//! tenant, When read, Then `plan.calls_per_day` carries `used`, `limit`,
//! `remaining`, `resets_at` and `remaining == limit - used`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn host_usage_plan_calls_per_day_carries_four_fields() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Meta Host Usage").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    for n in 0..3 {
        client.tools_call(&format!("{ns}.hello"), json!({"n": n})).await.expect("call");
    }

    let usage = client.tools_call("host.usage", json!({"window": "24h"})).await.expect("host.usage");
    let usage = extract_structured(&usage);
    let block = &usage["plan"]["calls_per_day"];
    let (used, limit, remaining) = (
        block["used"].as_i64().expect("used"),
        block["limit"].as_i64().expect("limit"),
        block["remaining"].as_i64().expect("remaining"),
    );
    assert!(block["resets_at"].as_str().is_some_and(|s| s.ends_with('Z')), "resets_at: {block}");
    assert_eq!(used, 3, "{block}");
    assert_eq!(remaining, limit - used, "{block}");
}
