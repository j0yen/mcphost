//! AC3 — Given a `free` tenant with 500 ok calls today, When it calls one
//! of its tools again, Then the call is rejected with `quota_exceeded`,
//! `limit.name: calls_per_day`, and a `resets_at` at the next UTC
//! midnight, and no `calls` row with `ok = 1` is written.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn echo_spec() -> serde_json::Value {
    json!({"schema": {"type": "object"}})
}

#[tokio::test]
async fn calls_over_the_daily_quota_are_rejected() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Heavy User").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": echo_spec()}),
        )
        .await
        .expect("publish must succeed");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");

    // Pre-seed the free plan's 500 ok calls today directly through the db
    // handle `TestServer` exposes -- driving 500 real HTTP round trips
    // through the whole MCP stack would make this test slow without
    // proving anything the seeded rows don't.
    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant.id, "echoer".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let qualified = format!("{ns}.echoer");
    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("the 501st call today must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    assert_eq!(err.data["plan"], json!("free"));
    assert_eq!(err.data["limit"]["name"], json!("calls_per_day"));
    assert_eq!(err.data["limit"]["value"], json!(500));
    assert_eq!(err.data["used"], json!(500));
    assert!(err.data["resets_at"].is_string(), "{:?}", err.data);
    assert_eq!(err.data["next"], json!("billing.checkout"));

    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let ok_calls_after = server
        .state
        .db
        .count_calls_since(tenant.id, midnight, true)
        .await
        .expect("count calls");
    assert_eq!(
        ok_calls_after, 500,
        "the rejected call must not have written an ok=1 calls row"
    );
}
