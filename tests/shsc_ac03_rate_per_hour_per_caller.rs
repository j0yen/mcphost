//! PRD-mcphost-scoped-tool-share
//! AC3 (P0) -- Given `rate_per_hour_per_caller: 2`, When one grantee calls
//! three times in a minute, Then the third returns `quota_caller` with
//! `limit: 2` and a `reset_at`, and a second grantee's first call succeeds.

use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, signup};

#[tokio::test]
async fn shsc_ac03_third_call_in_an_hour_is_quota_caller_and_other_grantee_is_unaffected() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Grantee B").await;
    let (ns_c, key_c) = signup(&server.base_url, "Grantee C").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let c = McpClient::with_bearer(&server.base_url, &key_c);

    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    owner.tools_call("host.group.create", json!({"name": "crew"})).await.expect("group create");
    for ns in [&ns_b, &ns_c] {
        owner
            .tools_call("host.group.add", json!({"name": "crew", "namespace": ns}))
            .await
            .expect("group add");
    }
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": "ping", "visibility": "group", "group": "crew", "rate_per_hour_per_caller": 2}),
        )
        .await
        .expect("share");

    let tool = format!("{ns_a}.ping");
    for n in 1..=2 {
        b.tools_call(&tool, json!({})).await.unwrap_or_else(|e| panic!("call {n}: {} {}", e.code, e.message));
    }
    let err = b.tools_call(&tool, json!({})).await.expect_err("third call must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("quota_caller"), "{}", err.message);
    assert_eq!(err.data["limit"], json!(2));
    assert!(err.data["reset_at"].as_str().is_some_and(|s| !s.is_empty()), "reset_at: {}", err.data);

    c.tools_call(&tool, json!({})).await.expect("a second grantee's first call succeeds");
}
