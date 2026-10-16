//! mcphost-js-kind AC5 (P0) — Given `timeout_s: 1` and `while(true){}`, When called, Then
//! `tool_timeout` within 2 s and the next call succeeds; Given
//! `memory_mb: 16` and an array growing without bound, Then `tool_oom`.

use crate::common;
use common::{TestServer, js_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn a_busy_loop_is_tool_timeout_within_two_seconds_and_the_next_call_succeeds() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (ns, key) = signup(&server.base_url, "JSK AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "function main(args) {\n  if (args.spin) { while (true) {} }\n  return { ok: true };\n}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 1,
    });
    client
        .tools_call("host.tool_publish", json!({"name": "spinner", "kind": "js", "spec": spec}))
        .await
        .expect("publish ok");

    let err = crate::deadline_budget!(2000, {
        client
            .tools_call(&format!("{ns}.spinner"), json!({"spin": true}))
            .await
            .expect_err("a call past its timeout must fail")
    });
    assert_eq!(err.error_code.as_deref(), Some("tool_timeout"), "{err:?}");

    let next = client
        .tools_call(&format!("{ns}.spinner"), json!({"spin": false}))
        .await
        .expect("the next call must succeed");
    assert!(next.to_string().contains("true"), "{next}");
}

#[tokio::test]
async fn an_array_growing_past_memory_mb_is_tool_oom() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (ns, key) = signup(&server.base_url, "JSK AC5b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "function main(args) {\n  const a = [];\n  while (true) { a.push(new Array(1000).fill(1)); }\n}\n",
        "args_schema": {"type": "object"},
        "memory_mb": 16,
    });
    client
        .tools_call("host.tool_publish", json!({"name": "hog", "kind": "js", "spec": spec}))
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.hog"), json!({}))
        .await
        .expect_err("growing past memory_mb must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tool_oom"), "{err:?}");
}
