//! PRD-mcphost-tool-test AC6 — Given a tenant at its daily call quota, When
//! it calls `host.spec_test`, Then the call is refused exactly as a normal
//! tool call is, and Given a tenant under quota, each executed invocation
//! increments the tenant's metered call count by one.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn spec_test_over_the_daily_quota_is_refused_like_a_normal_call() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Quota Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(_ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");

    // Pre-seed the free plan's 500 ok calls today directly through the db
    // handle, same shortcut `billing_ac03_call_time_quota_exceeded.rs` uses
    // -- driving 500 real round trips would prove nothing extra.
    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant.id, "echoer".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let err = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "invocations": [{}],
            }),
        )
        .await
        .expect_err("the 501st call-equivalent today must be rejected");

    // Same shape `host.tool_call`/a direct namespaced call gets at quota
    // (see billing_ac03_call_time_quota_exceeded.rs).
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    assert_eq!(err.data["plan"], json!("free"));
    assert_eq!(err.data["limit"]["name"], json!("calls_per_day"));
    assert_eq!(err.data["limit"]["value"], json!(500));
    assert_eq!(err.data["used"], json!(500));

    // The rejected call ran zero invocations: no additional metered row.
    let ok_calls_after = server
        .state
        .db
        .count_calls_since(
            tenant.id,
            mcphost::state::utc_midnight_unix(mcphost::state::now_unix()),
            true,
        )
        .await
        .expect("count calls");
    assert_eq!(
        ok_calls_after, 500,
        "a quota-refused host.spec_test call must not write a calls row"
    );
}

#[tokio::test]
async fn each_executed_invocation_increments_the_metered_call_count() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Metering Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Under quota (a fresh tenant has made zero calls): two invocations of
    // an echo spec should meter as two calls, same as two real published
    // calls would.
    let result = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "invocations": [{"a": 1}, {"a": 2}],
            }),
        )
        .await
        .expect("spec_test ok under quota");
    let invocations = extract_structured(&result)["invocations"]
        .as_array()
        .cloned()
        .expect("invocations array");
    assert_eq!(invocations.len(), 2);
    assert!(invocations.iter().all(|inv| inv["ok"] == json!(true)));

    let usage = extract_structured(
        &client
            .tools_call("host.usage", json!({}))
            .await
            .expect("host.usage"),
    );
    assert_eq!(
        usage["calls"].as_i64(),
        Some(2),
        "each of the two executed test invocations must count as a metered call: {usage}"
    );
}
