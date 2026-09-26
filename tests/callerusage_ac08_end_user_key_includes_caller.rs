//! PRD-mcphost-shared-tool-caller-usage
//! AC8 (P1) — Given a shared tool with end users across callers A and B,
//! When `by: "end_user"` runs for the owner, Then each key includes the
//! caller tenant and subject.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn end_user_keys_carry_the_caller_tenant_when_shared() {
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

    // Same subject ("u1"), two different callers -- each must be its own
    // row (requirement 5: the caller tenant is folded into the key), not
    // merged into one "u1" row the way a same-tenant identified call
    // (AC2) would be.
    server
        .state
        .db
        .record_call_attributed_with_end_user(
            owner.id, "tool_t".to_string(), 10, true, None, None, None, "ok",
            "external".to_string(), None, Some(tenant_a.id), Some("u1".to_string()), None,
            Some("assertion".to_string()),
        )
        .await
        .expect("seed A/u1 call");
    server
        .state
        .db
        .record_call_attributed_with_end_user(
            owner.id, "tool_t".to_string(), 10, true, None, None, None, "ok",
            "external".to_string(), None, Some(tenant_b.id), Some("u1".to_string()), None,
            Some("assertion".to_string()),
        )
        .await
        .expect("seed B/u1 call");

    let usage = client_o
        .tools_call("host.usage", json!({"tool": "tool_t", "by": "end_user", "window": "1d"}))
        .await
        .expect("host.usage by end_user");
    let body = extract_structured(&usage);
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2, "A's u1 and B's u1 must be distinct rows: {rows:?}");

    for ns in [ns_a.as_str(), ns_b.as_str()] {
        let expected_key = format!("{ns}:u1");
        let row = rows
            .iter()
            .find(|r| r["key"] == json!(expected_key))
            .unwrap_or_else(|| panic!("row with key '{expected_key}' must be present: {rows:?}"));
        assert_eq!(row["calls"], json!(1), "{row}");
    }
}
