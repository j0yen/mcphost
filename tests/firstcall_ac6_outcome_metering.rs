//! AC6 — Given a call that waited or returned `building`, When the calls
//! table is read, Then its outcome is recorded as `waited` or `building`,
//! distinct from `ok` (PRD-mcphost-first-call-reliability requirement 6,
//! P1).
//!
//! The `waited`/`building` write paths themselves are covered at the unit
//! level in `src/kinds/python.rs` (`*_firstcall_ac1`/`*_firstcall_ac2`),
//! which assert the exact signals (`waited_ms=...` log line,
//! `status: "building"` result shape) that `handler.rs`'s
//! `call_published_tool` reads to pick `record_call`'s `outcome` argument --
//! building a real slow environment here would make this suite depend on
//! wall-clock `uv` build time. This test instead proves the write/read
//! path end to end for the plain `ok` case (an echo call never waits or
//! builds anything) and that `outcome` is a real, distinct column, not a
//! derived/absent value.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn ordinary_call_records_ok_outcome_distinct_from_absent() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "OutcomeChecker").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "pinger",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("publish should succeed");

    let qualified = format!("{tenant_ns}.pinger");
    client
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("call should succeed");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let outcome = server
        .state
        .db
        .last_call_outcome(tenant.id, "pinger".to_string())
        .await
        .expect("last_call_outcome")
        .expect("a calls row must exist for this tool");
    assert_eq!(
        outcome, "ok",
        "an ordinary call that never waited or hit a building environment must \
         record outcome = ok, distinct from waited/building"
    );
    assert_ne!(outcome, "waited");
    assert_ne!(outcome, "building");
}
