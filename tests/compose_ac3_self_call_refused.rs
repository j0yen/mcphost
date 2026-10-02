//! PRD-mcphost-composition AC3 (P0) — Given a tool that calls itself, When
//! called, Then `compose_self_call` and no child run.
//!
//! No `runs` table exists yet (PRD-mcphost-runs-and-jobs, this PRD's
//! declared dependency, hasn't shipped), so "no child run" can't be
//! checked against a ledger; this asserts the observable half -- the call
//! is refused with `compose_self_call`.
//!
//! PRD-mcphost-chain-host-steps requirement 4 (landed after this PRD):
//! `host.tool_publish` now resolves every step's tool before publishing,
//! so "loopy" naming itself on its FIRST publish no longer resolves (the
//! name doesn't exist as a tool yet) -- published once against a
//! placeholder step instead, then republished naming itself, which now
//! resolves (the bare name "loopy" is itself a tool by then, from the
//! first publish). `compose_self_call`'s own refusal is still a pure
//! call-time check (`ctx.tool_name == target_name`), so the republished
//! self-reference still hits it exactly as before.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_chain_step_naming_its_own_chain_is_refused_with_compose_self_call() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Self Call Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    publish(
        &client,
        "placeholder",
        "echo",
        json!({"schema": {"type": "object"}}),
    )
    .await;
    publish(
        &client,
        "loopy",
        "chain",
        json!({"steps": [{"tool": "placeholder", "args": {}}]}),
    )
    .await;
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
