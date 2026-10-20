//! AC2 — Given tenant B with no trigger on tenant A's `push_reactor_1`, When
//! B calls `host.bridge_test` with A's hook URL, Then the error is
//! `host_not_allowed`, `data.hint` names `host.trigger.test` and
//! `host.trigger.replay`, and `data.trigger` is absent.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn ohur_ac02() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::new("mcphost.dev").expect("http kind")));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns_a, key_a) = signup(&server.base_url, "OHUR A").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    a.tools_call(
        "host.tool_publish",
        json!({"name": "push_reactor_1", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");
    a.tools_call("host.secret_set", json!({"name": "hook-secret-1", "value": "s3cr3t"}))
        .await
        .expect("secret_set");
    a.tools_call(
        "host.trigger.set",
        json!({"tool": "push_reactor_1", "kind": "event", "name": "push_event_1", "verify": "github", "secret": "hook-secret-1"}),
    )
    .await
    .expect("trigger.set");

    let (_ns_b, key_b) = signup(&server.base_url, "OHUR B").await;
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let spec = json!({"method": "POST", "url": format!("https://mcphost.dev/hooks/{ns_a}/push_reactor_1")});
    let err = b
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("own-domain url must be refused");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"), "{err:?}");
    let hint = err.data["hint"].as_str().expect("hint");
    assert!(hint.contains("host.trigger.test"), "{hint}");
    assert!(hint.contains("host.trigger.replay"), "{hint}");
    assert!(!hint.contains("push_event_1"), "{hint}");
    assert!(err.data.get("trigger").is_none(), "{:?}", err.data);
}
