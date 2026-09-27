//! PRD-mcphost-shared-call-run-scope
//! AC3 — Given the same call as AC1/AC2 (B's synchronous call on O's shared
//! `lookup`), When O's meter events and `host.usage` are read, Then exactly
//! one call is counted against O's `lookup`, as before this PRD -- proving
//! requirement 2 (the `calls` insert and metering emission are untouched by
//! requirement 1's `runs` row scoping change).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;
use serde_json::json;

#[tokio::test]
async fn owner_still_meters_exactly_one_call_for_the_shared_sync_call() {
    let server = TestServer::start().await;

    let (ns_o, key_o, owner_id) =
        signup_and_make_pro(&server, "Owner", "cus_sharedrunscope_ac3").await;
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
    let qualified = format!("{ns_o}.lookup");
    client_b
        .tools_call("host.tool_call", json!({"name": qualified, "args": {"msg": "hi"}}))
        .await
        .expect("B calls O's shared tool synchronously");

    let usage = extract_structured(
        &client_o
            .tools_call("host.usage", json!({}))
            .await
            .expect("O reads its own usage"),
    );
    assert_eq!(usage["calls"], json!(1), "O's calls row must exist: {usage}");
    assert_eq!(
        usage["calls_by_others"][&ns_b],
        json!(1),
        "O's calls_by_others must attribute the call to B: {usage}"
    );

    let fake = FakeBillingClient::new(mcphost::state::now_unix());
    let outcome = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("emit-meter run");
    assert_eq!(outcome.calls_covered, 1, "exactly one call must be metered: {outcome:?}");
    assert_eq!(outcome.groups.len(), 1);
    assert_eq!(outcome.groups[0].tenant_id, owner_id, "the metered call belongs to O, not B");
    assert_eq!(outcome.groups[0].count, 1);

    let ledger_rows = server.state.db.count_meter_events(Some(owner_id)).await.unwrap();
    assert_eq!(ledger_rows, 1, "exactly one meter_events ledger row for O");
}
