//! AC3 — Given tenant A with a webhook inbox bound to trigger `wh_1`, When A
//! tests `https://<own_domain>/hook/<id>`, Then `data.hint` contains
//! `host.trigger.test name=wh_1`; an inbox with no bound trigger says `no
//! trigger bound to inbox <id>`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::{Value, json};
use std::sync::Arc;

async fn refusal(client: &McpClient, url: String) -> Value {
    let spec = json!({"method": "POST", "url": url});
    let err = client
        .tools_call("host.bridge_test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("own-domain url must be refused");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"), "{err:?}");
    err.data
}

#[tokio::test]
async fn ohur_ac03() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::new("mcphost.dev").expect("http kind")));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "OHUR A").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "on_hook", "kind": "webhook", "name": "wh_1"}))
            .await
            .expect("trigger.set"),
    );
    let url = set["url"].as_str().expect("webhook url");
    let id = url.rsplit('/').next().expect("id").to_string();

    let data = refusal(&client, format!("https://mcphost.dev/hook/{id}")).await;
    let hint = data["hint"].as_str().expect("hint");
    assert!(hint.contains("host.trigger.test name=wh_1"), "{hint}");
    assert_eq!(data["trigger"], json!({"name": "wh_1", "kind": "webhook", "tool": "on_hook"}));

    let data = refusal(&client, "https://mcphost.dev/hook/01UNBOUNDINBOX".to_string()).await;
    let hint = data["hint"].as_str().expect("hint");
    assert!(hint.contains("no trigger bound to inbox 01UNBOUNDINBOX"), "{hint}");
    assert!(data.get("trigger").is_none(), "{data:?}");
}
