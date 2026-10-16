//! mcphost-js-kind AC7 (P0) — Given a spec with `secrets: ["API_KEY"]` whose tool returns the
//! secret, When `host.tool_test` runs, Then the value is redacted in `result`
//! and `host.tool_logs`.

use crate::common;
use common::{TestServer, extract_structured, js_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const SECRET_VALUE: &str = "sk-live-jsk7-never-leaves-4c1d9e";

#[tokio::test]
async fn a_returned_secret_is_redacted_in_tool_test_result_and_tool_logs() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "JSK AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.secret_set", json!({"name": "API_KEY", "value": SECRET_VALUE}))
        .await
        .expect("secret_set ok");

    let spec = json!({
        "source": "function main(args) {\n  const key = process.env.SECRET_API_KEY;\n  console.log(\"using \" + key);\n  return { key: key, header: \"Bearer \" + key };\n}\n",
        "args_schema": {"type": "object"},
        "secrets": ["API_KEY"],
    });
    client
        .tools_call("host.tool_publish", json!({"name": "leaker", "kind": "js", "spec": spec}))
        .await
        .expect("publish ok");

    let tested = client
        .tools_call("host.tool_test", json!({"name": "leaker", "args": {}}))
        .await
        .unwrap_or_else(|e| panic!("host.tool_test must succeed: {} {}", e.code, e.message));
    let tested_text = tested.to_string();
    assert!(!tested_text.contains(SECRET_VALUE), "secret leaked in host.tool_test: {tested_text}");
    let structured = extract_structured(&tested);
    let result = &structured["result"];
    assert_eq!(result["key"], json!("***"), "result.key must be redacted: {structured}");
    assert_eq!(result["header"], json!("Bearer ***"), "result.header must be redacted: {structured}");

    let logs = client
        .tools_call("host.tool_logs", json!({"name": "leaker"}))
        .await
        .expect("host.tool_logs");
    let logs_text = logs.to_string();
    assert!(!logs_text.contains(SECRET_VALUE), "secret leaked in host.tool_logs: {logs_text}");
}

#[tokio::test]
async fn a_secret_in_a_thrown_message_is_redacted_in_the_error_and_tool_logs() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (ns, key) = signup(&server.base_url, "JSK AC7b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.secret_set", json!({"name": "API_KEY", "value": SECRET_VALUE}))
        .await
        .expect("secret_set ok");
    let spec = json!({
        "source": "function main(args) {\n  throw new Error(\"rejected key \" + process.env.SECRET_API_KEY);\n}\n",
        "args_schema": {"type": "object"},
        "secrets": ["API_KEY"],
    });
    client
        .tools_call("host.tool_publish", json!({"name": "thrower", "kind": "js", "spec": spec}))
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.thrower"), json!({}))
        .await
        .expect_err("the tool throws");
    assert!(!format!("{err:?}").contains(SECRET_VALUE), "secret leaked in the error: {err:?}");
    assert!(err.message.contains("rejected key ***"), "{}", err.message);

    let logs = client
        .tools_call("host.tool_logs", json!({"name": "thrower"}))
        .await
        .expect("host.tool_logs");
    assert!(!logs.to_string().contains(SECRET_VALUE), "secret leaked in host.tool_logs: {logs}");
}
