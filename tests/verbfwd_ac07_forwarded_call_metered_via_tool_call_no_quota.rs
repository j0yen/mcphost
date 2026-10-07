//! PRD-mcphost-tool-call-host-verb-forward
//! AC7 — Given a forwarded `host.trigger.set`, When usage is recorded,
//! Then `host_tool_usage` has one row with `via: "tool_call"` and the
//! tenant's tool-call quota counter is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn forwarded_trigger_set_is_metered_via_tool_call_and_leaves_quota_untouched() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish pinger");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let quota_before = server
        .state
        .db
        .count_calls_since(tenant.id, midnight, true)
        .await
        .expect("count_calls_since before");

    // This tenant has never called host.trigger.set before -- its first
    // host_tool_usage row for it is written by this very forwarded call,
    // so `via` can only be the value this call records.
    assert_eq!(
        server
            .state
            .db
            .host_tool_usage_via(tenant.id, "host.trigger.set".to_string())
            .await
            .expect("host_tool_usage_via before"),
        None,
        "host.trigger.set must have no usage row before the forwarded call"
    );

    let call = client
        .tools_call(
            "host.tool_call",
            json!({
                "name": "trigger.set",
                "args": {"tool": "pinger", "kind": "schedule", "schedule": "*/5 * * * *"},
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("forwarding host.trigger.set must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&call)["forwarded_to"], "host.trigger.set");

    let via = server
        .state
        .db
        .host_tool_usage_via(tenant.id, "host.trigger.set".to_string())
        .await
        .expect("host_tool_usage_via after")
        .expect("host_tool_usage row for host.trigger.set must now exist");
    assert_eq!(via, "tool_call", "the forwarded verb's usage row must record via: tool_call");

    let quota_after = server
        .state
        .db
        .count_calls_since(tenant.id, midnight, true)
        .await
        .expect("count_calls_since after");
    assert_eq!(
        quota_after, quota_before,
        "a forwarded host verb call must never touch the tenant's tool-call quota counter"
    );
}
