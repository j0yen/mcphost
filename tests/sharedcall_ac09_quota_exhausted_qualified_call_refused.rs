//! PRD-mcphost-shared-tool-call-path
//! AC9 — Given host.tool_call {name: "<O>.lookup"} under B's free-plan
//! quota exhausted, When called, Then the same quota error an own-tool
//! call returns is produced and the tool does not execute.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn qualified_host_tool_call_is_refused_once_the_callers_quota_is_exhausted() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "lookup", "visibility": "public", "description": "lookup"}),
        )
        .await
        .expect("O shares lookup publicly");

    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let tenant_b = server
        .state
        .db
        .find_tenant_by_namespace(ns_b.clone())
        .await
        .expect("db query")
        .expect("B must exist");

    // Pre-seed B's free-plan daily quota (500 calls_per_day) directly,
    // same "seed directly" rationale as billing_ac03_call_time_quota_exceeded.rs
    // -- driving 500 real calls through the whole stack proves nothing this
    // seeding doesn't.
    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant_b.id, "lookup".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let qualified = format!("{ns_o}.lookup");
    let err = client_b
        .tools_call("host.tool_call", json!({"name": qualified, "args": {"msg": "hi"}}))
        .await
        .expect_err("the 501st call today (cross-tenant, via host.tool_call) must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    assert_eq!(err.data["plan"], json!("free"));
    assert_eq!(err.data["limit"]["name"], json!("calls_per_day"));
    assert_eq!(err.data["limit"]["value"], json!(500));
    assert_eq!(err.data["used"], json!(500));

    // The tool must not have executed: no additional ok=1 row for B, and
    // O's own cross-tenant attribution count stays at zero.
    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let ok_calls_after = server
        .state
        .db
        .count_calls_since(tenant_b.id, midnight, true)
        .await
        .expect("count calls");
    assert_eq!(ok_calls_after, 500, "the rejected call must not have written an ok=1 calls row");

    let tenant_o = server
        .state
        .db
        .find_tenant_by_namespace(ns_o.clone())
        .await
        .expect("db query")
        .expect("O must exist");
    let by_others = server
        .state
        .db
        .calls_by_others(tenant_o.id, 86_400)
        .await
        .expect("calls_by_others");
    assert!(
        by_others.iter().all(|(ns, _)| ns != &ns_b),
        "O's calls_by_others must show no call from B: the tool never ran, got {by_others:?}"
    );
}
