//! PRD-mcphost-tool-call-host-verb-forward
//! AC5 — Given `host.tool_test name="host.trigger.set" args={…}`, When the
//! verb supports `dry_run`, Then the dry run result is returned with
//! `forwarded_to`; given a verb without a dry run, Then the response is
//! `unverifiable` with `call_instead` and nothing is created.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn host_trigger_set_gets_a_real_dry_run_through_tool_test() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish pinger");

    let call = client
        .tools_call(
            "host.tool_test",
            json!({
                "name": "host.trigger.set",
                "args": {"tool": "pinger", "kind": "schedule", "schedule": "*/5 * * * *"},
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("host.trigger.set must have a dry run: {} {}", e.code, e.message));
    let structured = extract_structured(&call);

    assert_eq!(
        structured["forwarded_to"], "host.trigger.set",
        "the dry run result must carry forwarded_to: {structured}"
    );
    assert_eq!(structured["created"], json!(true), "the dry run must report what WOULD be created: {structured}");
    assert_eq!(
        structured["dry_run"]["rolled_back"], json!(true),
        "the dry run envelope must say it was rolled back: {structured}"
    );

    let list = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("host.trigger.list"),
    );
    assert_eq!(
        list["triggers"].as_array().expect("triggers array").len(),
        0,
        "the dry run must create nothing for real: {list}"
    );
}

#[tokio::test]
async fn a_forwardable_verb_without_a_dry_run_is_unverifiable() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant B").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_test", json!({"name": "host.trigger.list", "args": {}}))
        .await
        .expect_err("a verb with no dry run must refuse host.tool_test");

    assert_eq!(err.error_code.as_deref(), Some("unverifiable"), "{err:?}");
    assert_eq!(err.data["call_instead"]["tool"], "host_trigger_list", "{:?}", err.data);
}
