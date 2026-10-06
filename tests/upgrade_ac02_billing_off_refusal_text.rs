//! PRD-mcphost-upgrade-moment AC2 — Given billing mode off, When any plan
//! refusal occurs, Then `next.tool` is null, `why` and `resets_at` are
//! present, and the text says billing is not enabled on this host.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn echo_spec() -> serde_json::Value {
    json!({"schema": {"type": "object"}})
}

#[tokio::test]
async fn calls_per_day_refusal_with_billing_off_names_no_checkout_tool() {
    // `TestServer::start()` defaults to `BillingConfig::default()` (no
    // secret key configured), i.e. billing mode off.
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Self Hoster").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": echo_spec()}),
        )
        .await
        .expect("publish must succeed");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");

    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant.id, "echoer".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let qualified = format!("{ns}.echoer");
    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("the 501st call today must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    let next = &err.data["next"];
    assert_eq!(next["tool"], serde_json::Value::Null, "{next:?}");
    assert_eq!(next["why"], json!("calls_per_day 500/500"), "{next:?}");
    assert!(next["resets_at"].is_string(), "{next:?}");

    assert!(
        err.message.contains("billing is not enabled on this host"),
        "refusal text must say billing is off: {}",
        err.message
    );
}
