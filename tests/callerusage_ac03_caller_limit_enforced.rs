//! PRD-mcphost-shared-tool-caller-usage
//! AC3 (P0) — Given `host.share.caller_limit {tool: "tool_t", caller_tenant: "A",
//! calls_per_day: 3}`, When A makes a 4th call within the day, Then it
//! returns `quota_caller` with `reset_at` and no meter event with a
//! successful outcome is written; B is unaffected.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn fourth_call_over_the_caller_limit_is_rejected_b_unaffected() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner O").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_t", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes T");
    client_o
        .tools_call("host.tool_share", json!({"name": "tool_t", "visibility": "public"}))
        .await
        .expect("O shares T publicly");

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_o
        .tools_call(
            "host.share.caller_limit",
            json!({"tool": "tool_t", "caller_tenant": ns_a, "calls_per_day": 3}),
        )
        .await
        .expect("O sets A's caller_limit to 3");

    let owner = server
        .state
        .db
        .find_tenant_by_namespace(ns_o.clone())
        .await
        .expect("db query")
        .expect("owner exists");
    let tenant_a = server
        .state
        .db
        .find_tenant_by_namespace(ns_a.clone())
        .await
        .expect("db query")
        .expect("A exists");

    // Pre-seed A's first 3 (of its 4 total) calls -- same "seed the
    // precondition directly, drive only the call under test through the
    // real stack" convention as tests/billing_ac03_call_time_quota_exceeded.rs.
    for _ in 0..3 {
        server
            .state
            .db
            .record_call_attributed(
                owner.id,
                "tool_t".to_string(),
                10,
                true,
                None,
                None,
                None,
                "ok",
                "external".to_string(),
                None,
                Some(tenant_a.id),
            )
            .await
            .expect("seed A call");
    }

    let qualified = format!("{ns_o}.tool_t");
    let err = client_a
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("A's 4th call today must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("quota_caller"));
    assert_eq!(err.data["caller_tenant"], json!(ns_a));
    assert_eq!(err.data["tool"], json!("tool_t"));
    assert_eq!(err.data["limit"], json!(3));
    assert_eq!(err.data["used"], json!(3));
    assert!(err.data["reset_at"].is_string(), "{:?}", err.data);

    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let a_calls_after = server
        .state
        .db
        .count_caller_calls_since(owner.id, "tool_t".to_string(), tenant_a.id, midnight)
        .await
        .expect("count A's calls");
    assert_eq!(
        a_calls_after, 3,
        "the rejected 4th call must not have written a successful calls row"
    );

    // B is unaffected: no caller_limit was set for B, so its call succeeds.
    let b_result = client_b.tools_call(&qualified, json!({})).await;
    assert!(
        b_result.is_ok(),
        "B's call must succeed (no caller_limit set for B): {b_result:?}"
    );

    let usage = client_o
        .tools_call(
            "host.usage",
            json!({"tool": "tool_t", "by": "caller", "window": "1d"}),
        )
        .await
        .expect("host.usage by caller");
    let body = extract_structured(&usage);
    let rows = body["rows"].as_array().expect("rows array");
    let a_row = rows
        .iter()
        .find(|r| r["key"] == json!(ns_a))
        .expect("A's row must be present");
    assert_eq!(a_row["calls"], json!(3), "A's successful calls must still be 3: {a_row}");
}
