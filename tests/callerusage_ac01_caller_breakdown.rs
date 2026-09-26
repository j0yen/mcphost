//! PRD-mcphost-shared-tool-caller-usage
//! AC1 (P0) — Given owner O shares tool T with tenants A and B, and A calls
//! it 5 times and B 2 times, When O calls
//! `host.usage {tool: "tool_t", by: "caller", window: "1d"}`, Then rows for A
//! (5) and B (2) return with error counts and p95.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn caller_breakdown_reports_each_callers_count() {
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

    let (ns_a, _key_a) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Tenant B").await;
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
    let tenant_b = server
        .state
        .db
        .find_tenant_by_namespace(ns_b.clone())
        .await
        .expect("db query")
        .expect("B exists");

    // Seeded directly (same convention as tests/billing_ac03_call_time_
    // quota_exceeded.rs): the cross-tenant attribution write path is
    // already proven by tests/share_ac05_usage_and_quota_attribution.rs;
    // this AC is about the READ side.
    for _ in 0..5 {
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
    for _ in 0..2 {
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
                Some(tenant_b.id),
            )
            .await
            .expect("seed B call");
    }

    let usage = client_o
        .tools_call(
            "host.usage",
            json!({"tool": "tool_t", "by": "caller", "window": "1d"}),
        )
        .await
        .expect("host.usage by caller");
    let body = extract_structured(&usage);
    assert_eq!(body["by"], json!("caller"));
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2, "expected exactly A and B rows: {rows:?}");

    for (ns, expected_calls) in [(ns_a.as_str(), 5), (ns_b.as_str(), 2)] {
        let row = rows
            .iter()
            .find(|r| r["key"] == json!(ns))
            .unwrap_or_else(|| panic!("row for {ns} must be present: {rows:?}"));
        assert_eq!(row["calls"], json!(expected_calls), "{row}");
        assert_eq!(row["errors"], json!(0), "{row}");
        assert!(row["p95_ms"].is_number(), "{row}");
    }
}
