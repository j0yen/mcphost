//! PRD-mcphost-call-trace-replay
//! AC2 -- Given any `host.tool_call`, `host.tool.call`, `host.tool.run` or
//! `host.tool.test` result, When read, Then it carries `call_id` as a string
//! equal to the `calls.id` of the row written for it.

use crate::ctrace;
use crate::common;

use common::extract_structured;
use mcphost::sandbox;
use serde_json::{Value, json};

/// The `call_id` of `result`, asserted to be a string naming the newest
/// `calls` row of `tenant_id` -- and that row's `tool_name`.
async fn call_id_is_the_newest_row(fx: &ctrace::Fixture, tenant_id: i64, result: &Value) -> (String, String) {
    let call_id = result["call_id"].as_str().unwrap_or_else(|| panic!("call_id must be a string: {result}")).to_string();
    let db = &fx.server.state.db;
    let newest = db.latest_call_id_for_test(tenant_id).await.expect("latest call").expect("a calls row exists");
    assert_eq!(call_id, newest.to_string(), "call_id must be the calls.id of the row just written: {result}");
    let tool_name = db
        .call_tool_name_for_test(newest)
        .await
        .expect("tool_name lookup")
        .expect("the row exists");
    (call_id, tool_name)
}

#[tokio::test]
async fn every_tool_call_test_and_run_result_carries_the_calls_id_as_a_string() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    let tenant_id = fx.tenant_id().await;
    fx.publish_python("echo", "def main(args):\n    return {\"n\": args.get(\"n\", 0)}\n").await;
    fx.warm("echo", json!({"n": 0})).await;

    // host.tool_call
    let result = fx.tool_call("echo", json!({"n": 1})).await.expect("host.tool_call");
    assert_eq!(result["n"], json!(1), "the tool's own output is still there: {result}");
    let (first, tool_name) = call_id_is_the_newest_row(&fx, tenant_id, &result).await;
    assert_eq!(tool_name, "echo");

    // host.tool.call, the canonical spelling
    let canonical = extract_structured(
        &fx.client
            .tools_call("host.tool.call", json!({"name": "echo", "args": {"n": 2}}))
            .await
            .expect("host.tool.call"),
    );
    let (second, tool_name) = call_id_is_the_newest_row(&fx, tenant_id, &canonical).await;
    assert_eq!(tool_name, "echo");
    assert_ne!(first, second, "each call writes its own row");

    // host.tool.test: a dry run writes a (non-metered) row too.
    let tested = extract_structured(
        &fx.client
            .tools_call("host.tool.test", json!({"name": "echo", "args": {"n": 3}}))
            .await
            .expect("host.tool.test"),
    );
    let (_, tool_name) = call_id_is_the_newest_row(&fx, tenant_id, &tested).await;
    assert_eq!(tool_name, "__tool_test__.echo");

    // host.tool.run
    let ran = extract_structured(
        &fx.client
            .tools_call("host.tool.run", json!({"name": "echo", "args": {"n": 4}}))
            .await
            .expect("host.tool.run"),
    );
    let (_, tool_name) = call_id_is_the_newest_row(&fx, tenant_id, &ran).await;
    assert_eq!(tool_name, "__tool_test__.echo");

    // A failed call has no result object: the id rides in the error data.
    fx.publish_python("boom", "def main(args):\n    raise ValueError('nope')\n").await;
    fx.warm("boom", json!({})).await;
    let err = fx.tool_call("boom", json!({})).await.expect_err("boom must fail");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{err:?}");
    let failed_id = err.data["call_id"].as_str().unwrap_or_else(|| panic!("call_id in error data: {err:?}")).to_string();
    let newest = fx.server.state.db.latest_call_id_for_test(tenant_id).await.expect("latest").expect("row");
    assert_eq!(failed_id, newest.to_string());

    // The test and run rows are not metered: host.usage counts only the
    // three real calls (two echo, one boom) plus the warm-up calls.
    let usage = extract_structured(&fx.client.tools_call("host.usage", json!({})).await.expect("host.usage"));
    let counted = usage["calls"].as_i64().expect("calls count");
    let real_rows = fx
        .server
        .state
        .db
        .count_calls_since(tenant_id, 0, false)
        .await
        .expect("count");
    assert_eq!(counted, real_rows, "usage and the quota count the same rows: {usage}");
    assert!(counted < newest, "the test/run rows are in calls but not counted: counted {counted}, newest id {newest}");
}
