//! PRD-mcphost-composition AC3 (P0) — Given a tool that calls itself, When
//! called, Then `compose_self_call` and no child run.
//!
//! No `runs` table exists yet (PRD-mcphost-runs-and-jobs, this PRD's
//! declared dependency, hasn't shipped), so "no child run" can't be
//! checked against a ledger; this asserts the observable half -- the call
//! is refused with `compose_self_call`. A `chain`'s `validate` never
//! resolves step names, so "loopy" naming itself as its own step publishes
//! fine; the refusal only happens at call time.

mod common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_chain_step_naming_its_own_chain_is_refused_with_compose_self_call() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Self Call Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let qualified = publish(
        &client,
        "loopy",
        "chain",
        json!({"steps": [{"tool": "loopy", "args": {}}]}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.loopy"));

    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("a tool naming itself as a step must be refused");
    assert_eq!(err.error_code.as_deref(), Some("compose_self_call"));
}
