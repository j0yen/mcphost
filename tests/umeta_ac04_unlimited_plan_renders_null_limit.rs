//! PRD-mcphost-usage-in-every-response AC4 -- Given a plan row with
//! `calls_per_day: 0`, When a call succeeds, Then
//! `_meta.usage.calls_per_day.limit` and `remaining` are null and `used` is
//! the count.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn zero_calls_per_day_renders_null_limit_and_remaining() {
    let server = TestServer::start_with_plans(|catalog| {
        for plan in catalog.plans.iter_mut().filter(|p| p.name == "free") {
            plan.calls_per_day = 0;
        }
    })
    .await;
    let (ns, key) = signup(&server.base_url, "Meta Unlimited").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    for n in 1..=2 {
        let result = client.tools_call(&format!("{ns}.hello"), json!({"n": n})).await.expect("unlimited plan call succeeds");
        let block = &result["_meta"]["usage"]["calls_per_day"];
        assert_eq!(block["used"].as_i64(), Some(n), "{result}");
        assert_eq!(block["limit"], Value::Null, "{result}");
        assert_eq!(block["remaining"], Value::Null, "{result}");
    }
}
