//! AC1 — Given tenant A with event trigger `push_event_1` on tool
//! `push_reactor_1`, When A calls `host.bridge_test` with its own
//! `/hooks/<ns>/push_reactor_1` URL, Then the error is `host_not_allowed`,
//! `data.hint` contains `host.trigger.test name=push_event_1` and
//! `data.trigger` names the trigger.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn ohur_ac01() {
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

    let spec = json!({"method": "POST", "url": format!("https://mcphost.dev/hooks/{ns}/push_reactor_1")});
    let err = client
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("own-domain url must be refused");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"), "{err:?}");
    let hint = err.data["hint"].as_str().expect("hint");
    assert!(hint.contains("host.trigger.test name=push_event_1"), "{hint}");
    assert_eq!(
        err.data["trigger"],
        json!({"name": "push_event_1", "kind": "event", "tool": "push_reactor_1"})
    );
}
