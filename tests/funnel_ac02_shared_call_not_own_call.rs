//! PRD-mcphost-activation-funnel
//! AC2 (P0) — Given a tenant that calls a tool shared by another tenant,
//! When `admin.funnel` is read, Then that call does not count as
//! `first_own_call`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn cross_tenant_shared_call_never_counts_as_first_own_call() {
    let server = TestServer::start().await;

    // Two external tenants (AC2's own scenario needs no particular
    // source_class, but `control::signup` with a real IP is this suite's
    // precedent for a non-loopback tenant -- see
    // attrib_ac4_external_signup_counts_real.rs).
    let owner = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Owner"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("owner signup");
    let owner_ns = owner["tenant"].as_str().expect("owner namespace").to_string();
    let owner_key = owner["key"].as_str().expect("owner key").to_string();

    let caller = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Caller"}),
        "8.8.8.9",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("caller signup");
    let caller_key = caller["key"].as_str().expect("caller key").to_string();

    let owner_client = McpClient::with_bearer(&server.base_url, &owner_key);
    owner_client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "funnel_ac02_tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}},
            }),
        )
        .await
        .expect("owner publishes");
    owner_client
        .tools_call("host.tool_share", json!({"name": "funnel_ac02_tool", "visibility": "public"}))
        .await
        .expect("owner shares publicly");

    let caller_client = McpClient::with_bearer(&server.base_url, &caller_key);
    let qualified = format!("{owner_ns}.funnel_ac02_tool");
    caller_client
        .tools_call(&qualified, json!({"msg": "borrowed"}))
        .await
        .expect("caller calls the shared tool");

    // Neither tenant's own `first_own_call_unix` is set by that call.
    let owner_tenant = server
        .state
        .db
        .find_tenant_by_namespace(owner_ns)
        .await
        .expect("query")
        .expect("owner exists");
    assert!(
        owner_tenant.first_own_call_unix.is_none(),
        "the owner never called its own tool, only the caller did: {owner_tenant:?}"
    );
    let caller_ns = caller["tenant"].as_str().expect("caller namespace").to_string();
    let caller_tenant = server
        .state
        .db
        .find_tenant_by_namespace(caller_ns)
        .await
        .expect("query")
        .expect("caller exists");
    assert!(
        caller_tenant.first_own_call_unix.is_none(),
        "a cross-tenant shared-tool call is never the caller's own first_own_call either: {caller_tenant:?}"
    );

    // And `admin.funnel` agrees: the `first_own_call` stage counts neither
    // of these two signups.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = extract_structured(
        &admin.tools_call("admin.funnel", json!({"days": 7})).await.expect("admin.funnel"),
    );
    let stages = funnel["stages"].as_array().expect("stages array");
    let first_own_call = stages
        .iter()
        .find(|s| s["name"] == json!("first_own_call"))
        .expect("first_own_call stage present");
    assert_eq!(
        first_own_call["count"], json!(0),
        "a shared-tool call must never count as first_own_call: {funnel}"
    );
}
