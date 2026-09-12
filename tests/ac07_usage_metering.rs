//! AC7 — Given 20 `tools/call` requests to a tool, When the tenant calls
//! `host.usage("24h")`, Then it reports 20 calls with p50 and p95
//! durations, and `admin.usage` with the admin key reports the same under
//! that tenant and tool.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn usage_reports_20_calls_to_both_tenant_and_admin() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Metered Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let qualified = format!("{tenant_ns}.hello");

    for i in 0..20 {
        client
            .tools_call(&qualified, json!({"n": i}))
            .await
            .unwrap_or_else(|e| panic!("call {i} failed: {e:?}"));
    }

    let usage = client
        .tools_call("host.usage", json!({"window": "24h"}))
        .await
        .expect("host.usage");
    let usage = extract_structured(&usage);
    assert_eq!(
        usage["calls"].as_i64(),
        Some(20),
        "expected 20 calls, got {usage}"
    );
    assert!(usage["p50_ms"].is_number());
    assert!(usage["p95_ms"].is_number());

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let admin_usage = admin
        .tools_call("admin.usage", json!({"window": "24h"}))
        .await
        .expect("admin.usage");
    let admin_usage = extract_structured(&admin_usage);
    let rows = admin_usage["usage"].as_array().expect("usage array");
    let row = rows
        .iter()
        .find(|r| r["tenant"] == json!(tenant_ns) && r["tool"] == json!("hello"))
        .unwrap_or_else(|| panic!("no admin.usage row for {tenant_ns}.hello in {rows:?}"));
    assert_eq!(row["calls"].as_i64(), Some(20));
}
