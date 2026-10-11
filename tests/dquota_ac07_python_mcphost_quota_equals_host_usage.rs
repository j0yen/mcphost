//! PRD-mcphost-data-quota-in-every-response
//! AC7 -- Given a python tool calling `mcphost.quota()`, When called via
//! `host.tool_call`, Then the object equals `host.usage.quota` for that
//! tenant at that moment.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn python_mcphost_quota_returns_the_host_usage_quota_object() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Quota").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.state.set", json!({"key": "k", "value": "some stored value"}))
        .await
        .expect("seed state");

    let spec = json!({
        "source": "import mcphost\ndef main(args):\n    return {\"quota\": mcphost.quota()}\n",
        "args_schema": {"type": "object", "properties": {}},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "quota_tool", "kind": "python", "spec": spec}))
        .await
        .expect("publish");

    let called = poll_until_ready(&client, &format!("{ns}.quota_tool"), json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let from_tool = extract_structured(&called)["quota"].clone();
    let usage = extract_structured(&client.tools_call("host.usage", json!({})).await.expect("usage"));
    assert!(from_tool.as_object().is_some_and(|o| !o.is_empty()), "{called:?}");
    assert_eq!(from_tool.to_string(), usage["quota"].to_string());
    assert!(from_tool["state_bytes_max"]["used"].as_i64().unwrap_or(0) > 0);
}
