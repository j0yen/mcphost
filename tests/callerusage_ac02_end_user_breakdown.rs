//! PRD-mcphost-shared-tool-caller-usage
//! AC2 (P0) — Given identified calls from u1 (3) and u2 (1) to O's tool,
//! When `host.usage {by: "end_user", window: "1d"}` runs, Then two rows
//! return with those counts.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn end_user_breakdown_reports_each_subjects_count() {
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
    let owner = server
        .state
        .db
        .find_tenant_by_namespace(ns_o.clone())
        .await
        .expect("db query")
        .expect("owner exists");

    // Seeded directly (the identity-verification pipeline itself --
    // OAuth sub / signed end_user_assertion -> EndUser -- is already
    // proven by tests/enduser_ac01_*/enduser_ac02_*; this AC is about the
    // READ side of the resulting calls rows).
    for _ in 0..3 {
        server
            .state
            .db
            .record_call_attributed_with_end_user(
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
                None,
                Some("u1".to_string()),
                None,
                Some("assertion".to_string()),
            )
            .await
            .expect("seed u1 call");
    }
    server
        .state
        .db
        .record_call_attributed_with_end_user(
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
            None,
            Some("u2".to_string()),
            None,
            Some("assertion".to_string()),
        )
        .await
        .expect("seed u2 call");

    let usage = client_o
        .tools_call("host.usage", json!({"by": "end_user", "window": "1d"}))
        .await
        .expect("host.usage by end_user");
    let body = extract_structured(&usage);
    assert_eq!(body["by"], json!("end_user"));
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2, "expected exactly u1 and u2 rows: {rows:?}");

    for (subject, expected_calls) in [("u1", 3), ("u2", 1)] {
        let row = rows
            .iter()
            .find(|r| r["key"] == json!(subject))
            .unwrap_or_else(|| panic!("row for {subject} must be present: {rows:?}"));
        assert_eq!(row["calls"], json!(expected_calls), "{row}");
    }
}
