//! PRD-mcphost-session-key
//! AC18 — Given a call carrying `tenant_key`, When `host.tool_logs` returns
//! lines for that invocation, Then the key value is absent from every
//! returned line.

mod common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_logs_never_contain_the_tenant_key_used_to_authenticate_the_call() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Logged Caller").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish");

    // Call over tenant_key (no header) so the key value is genuinely on the
    // wire for this invocation.
    let anon = McpClient::new(&server.base_url);
    anon.tools_call(
        "host.tool_call",
        json!({"name": "hello", "args": {}, "tenant_key": key}),
    )
    .await
    .expect("host.tool_call over tenant_key");

    let logs = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "hello"}))
            .await
            .expect("host.tool_logs"),
    );
    let dump = logs.to_string();
    assert!(
        !dump.contains(&key),
        "host.tool_logs must never contain the tenant_key value: {dump}"
    );
}
