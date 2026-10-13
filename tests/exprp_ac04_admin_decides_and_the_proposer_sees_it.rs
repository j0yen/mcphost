//! PRD-mcphost-example-from-published-tool
//! AC4 — Given a pending proposal, When `admin.example_proposals
//! {action: approve, id}` runs, Then status is `approved` with
//! `decided_by`; a non-admin bearer gets `forbidden`; the proposer's
//! `host.example.proposal_status` shows `approved`.

use crate::exprp;

use crate::common::{ADMIN_KEY, McpClient, extract_structured};
use serde_json::json;

async fn pending_proposal(f: &exprp::Fixture, tool: &str) -> i64 {
    exprp::publish_http(&f.client, tool, exprp::probe_spec(&f.upstream)).await;
    let p = exprp::propose(&f.client, exprp::propose_args(tool)).await.unwrap_or_else(|e| panic!("propose: {} {}", e.code, e.message));
    p["proposal_id"].as_i64().expect("proposal_id")
}

#[tokio::test]
async fn approve_records_the_decider_and_the_proposer_reads_approved() {
    let f = exprp::start().await;
    let id = pending_proposal(&f, "probe").await;
    let admin = McpClient::with_bearer(&f.server.base_url, ADMIN_KEY);

    let listed = extract_structured(&admin.tools_call("admin.example_proposals", json!({"action": "list"})).await.expect("list"));
    assert_eq!(listed["proposals"][0]["id"], id);
    assert_eq!(listed["proposals"][0]["status"], "pending");

    let approved = extract_structured(
        &admin.tools_call("admin.example_proposals", json!({"action": "approve", "id": id})).await.expect("approve"),
    );
    assert_eq!(approved["status"], "approved");
    assert!(approved["decided_by"].as_str().is_some_and(|d| !d.is_empty()), "decided_by: {approved}");
    assert!(approved["decided_at"].as_i64().is_some(), "decided_at: {approved}");

    let status = extract_structured(&f.client.tools_call("host.example.proposal_status", json!({"id": id})).await.expect("status"));
    assert_eq!(status["status"], "approved");
    assert_eq!(status["decided_by"], approved["decided_by"]);

    // R7: the proposer is told by inbox message.
    let inbox = extract_structured(&f.client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    let notice = inbox["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["data"]["proposal_id"].as_i64() == Some(id)))
        .unwrap_or_else(|| panic!("no proposal notice in {inbox}"))
        .clone();
    assert_eq!(notice["from_address"], "admin");
    assert!(notice["body"].as_str().unwrap_or_default().contains("approved"), "{notice}");

    // A decided proposal cannot be decided again.
    let again = admin.tools_call("admin.example_proposals", json!({"action": "reject", "id": id})).await.expect_err("already decided");
    assert_eq!(again.error_code.as_deref(), Some("args_invalid"), "{} {}", again.code, again.message);
}

#[tokio::test]
async fn a_tenant_bearer_is_forbidden_and_reject_carries_the_reason() {
    let f = exprp::start().await;
    let id = pending_proposal(&f, "probe").await;

    let err = f
        .client
        .tools_call("admin.example_proposals", json!({"action": "approve", "id": id}))
        .await
        .expect_err("a tenant key must never decide");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"), "{} {}", err.code, err.message);
    let still = extract_structured(&f.client.tools_call("host.example.proposal_status", json!({"id": id})).await.expect("status"));
    assert_eq!(still["status"], "pending");

    let admin = McpClient::with_bearer(&f.server.base_url, ADMIN_KEY);
    let rejected = extract_structured(
        &admin.tools_call("admin.example_proposals", json!({"action": "reject", "id": id, "reason": "too thin"})).await.expect("reject"),
    );
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["reason"], "too thin");
}

#[tokio::test]
async fn a_tenant_reads_only_its_own_proposals() {
    let f = exprp::start().await;
    let id = pending_proposal(&f, "probe").await;
    let (_ns, key) = crate::common::signup(&f.server.base_url, "exprp stranger").await;
    let stranger = McpClient::with_bearer(&f.server.base_url, &key);
    let err = stranger.tools_call("host.example.proposal_status", json!({"id": id})).await.expect_err("not theirs");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{} {}", err.code, err.message);
}
