//! AC5 — Given AC1's spec, When A calls `host.tool_publish` instead of
//! `host.bridge_test`, Then code, hint and `data.trigger` are identical.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn ohur_ac05() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::new("mcphost.dev").expect("http kind")));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "OHUR A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "push_reactor_1", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call("host.secret_set", json!({"name": "hook-secret-1", "value": "s3cr3t"}))
        .await
        .expect("secret_set");
    client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "push_reactor_1", "kind": "event", "name": "push_event_1", "verify": "github", "secret": "hook-secret-1"}),
        )
        .await
        .expect("trigger.set");

    let spec = json!({"method": "POST", "url": format!("https://mcphost.dev/hooks/{ns}/push_reactor_1"), "args_schema": {"type": "object"}});
    let tested = client
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("bridge_test refuses");
    let published = client
        .tools_call("host.tool_publish", json!({"name": "self_hook", "kind": "http", "spec": spec}))
        .await
        .expect_err("publish refuses");
    assert_eq!(published.error_code.as_deref(), Some("host_not_allowed"), "{published:?}");
    assert_eq!(published.error_code, tested.error_code);
    assert_eq!(published.data["hint"], tested.data["hint"]);
    assert_eq!(published.data["trigger"], tested.data["trigger"]);
    assert_eq!(
        published.data["trigger"],
        json!({"name": "push_event_1", "kind": "event", "tool": "push_reactor_1"})
    );
}
