//! PRD-mcphost-call-cost-estimate
//! AC4 -- Given 50 concurrent dry runs from one free tenant
//! (`concurrent_calls_per_tenant` 4), When they run, Then all 50 return and
//! no `capacity` refusal is produced, and a real call issued during them
//! succeeds.

use crate::common;
use crate::slotkind;

use common::{McpClient, TestServer, extract_structured, signup};
use futures_util::future::join_all;
use serde_json::json;
use slotkind::SlotKind;

#[tokio::test]
async fn fifty_dry_runs_take_no_slot_and_a_real_call_still_succeeds() {
    // Four admission slots: the free plan's `concurrent_calls_per_tenant`.
    let slot = SlotKind::new(4);
    let server = TestServer::start_with_kinds(slot.registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cest Concurrent").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "held", "kind": "slot", "spec": {"hold_ms": 300}}),
        )
        .await
        .expect("publish");

    // Two real calls hold two of the four slots and the real call under test
    // takes a third: one slot stays free, so any dry run that took a slot
    // (50 of them would exhaust all four) shows up as a `capacity` refusal.
    let holders: Vec<_> = (0..2)
        .map(|_| client.tools_call("host.tool_call", json!({"name": "held", "args": {}})))
        .collect();
    let dry_runs: Vec<_> = (0..50)
        .map(|_| client.tools_call("host.tool_call", json!({"name": "held", "args": {}, "dry_run": true})))
        .collect();
    let real = client.tools_call("host.tool_call", json!({"name": "held", "args": {}}));

    let (held, dry, real) = tokio::join!(join_all(holders), join_all(dry_runs), real);

    assert_eq!(dry.len(), 50);
    for (i, answer) in dry.into_iter().enumerate() {
        let body = extract_structured(&answer.unwrap_or_else(|e| panic!("dry run {i} errored: {e:?}")));
        assert_eq!(body["dry_run"], json!(true), "dry run {i}: {body}");
        assert_ne!(body["refusal"]["code"], json!("capacity"), "dry run {i} was refused on capacity: {body}");
    }
    for (i, answer) in held.into_iter().enumerate() {
        answer.unwrap_or_else(|e| panic!("holder call {i} failed: {e:?}"));
    }
    real.expect("a real call issued during the dry runs succeeds");
    assert_eq!(slot.semaphore().available_permits(), 4, "every slot is back once the calls end");
}
