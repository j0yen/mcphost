//! PRD-mcphost-usage-in-every-response AC6 -- Given a shared tool with a
//! caller cap of 10 and 4 calls made by the caller, When the caller's 5th
//! call succeeds, Then `_meta.usage.caller_limit` is `{used: 5, limit: 10}`
//! and the owner's result for its own call carries no `caller_limit`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn fifth_call_reports_caller_limit_and_owner_call_carries_none() {
    let server = TestServer::start().await;
    let (ns_o, key_o) = signup(&server.base_url, "Meta Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_o);
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_t", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    owner.tools_call("host.tool_share", json!({"name": "tool_t", "visibility": "public"})).await.expect("share");

    let (ns_c, key_c) = signup(&server.base_url, "Meta Caller").await;
    let caller = McpClient::with_bearer(&server.base_url, &key_c);
    owner
        .tools_call(
            "host.share.caller_limit",
            json!({"tool": "tool_t", "caller_tenant": ns_c, "calls_per_day": 10}),
        )
        .await
        .expect("set caller cap");

    let shared = format!("{ns_o}.tool_t");
    let mut last = json!(null);
    for n in 1..=5 {
        last = caller.tools_call(&shared, json!({"n": n})).await.unwrap_or_else(|e| panic!("caller call {n}: {e:?}"));
    }
    assert_eq!(
        last["_meta"]["usage"]["caller_limit"],
        json!({"used": 5, "limit": 10}),
        "caller's 5th call: {last}"
    );
    assert_eq!(last["_meta"]["usage"]["calls_per_day"]["used"], json!(5), "{last}");

    let own = owner.tools_call(&shared, json!({"own": true})).await.expect("owner calls its own tool");
    assert!(own["_meta"]["usage"]["calls_per_day"].is_object(), "owner still gets calls_per_day: {own}");
    assert!(own["_meta"]["usage"].get("caller_limit").is_none(), "owner result must carry no caller_limit: {own}");
}
