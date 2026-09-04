//! PRD-mcphost-session-key
//! AC11 — Given a tenant that has published `hello`, When it calls
//! `host.tool_call` with `name: "hello"` and valid `args` and its
//! `tenant_key`, Then the published tool executes and its result is
//! returned.
//! AC12 — Given the call in AC11, When `host.usage` is queried afterwards,
//! Then the call is counted, proving `host.tool_call` is metered unlike
//! `host.tool_test`.
//! AC13 — Given the call in AC11, When `host.tool_logs` is queried for
//! `hello`, Then the invocation appears in the returned lines.
//! AC14 — Given a tenant, When it calls `host.tool_call` with `args` that
//! violate the kind's input schema, Then the call fails with the
//! invalid-arguments error, matching the direct namespaced call path.
//! AC15 — Given tenant A and tenant B where only B published `hello`, When
//! A calls `host.tool_call` with `name: "hello"`, Then it receives
//! tool-not-found and nothing in the response or error reveals that B
//! published a tool by that name.

mod common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_call_executes_meters_and_appears_in_logs() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Caller").await;
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

    // AC11: host.tool_call with tenant_key (also carrying the header here,
    // which just takes precedence -- the point is host.tool_call itself
    // executes the real tool and returns its result).
    let call = client
        .tools_call(
            "host.tool_call",
            json!({"name": "hello", "args": {}, "tenant_key": key}),
        )
        .await
        .expect("host.tool_call");
    let structured = extract_structured(&call);
    assert_eq!(structured["status"], 200);
    assert_eq!(structured["body"]["ok"], true);

    // AC12: metered, unlike host.tool_test.
    let usage = extract_structured(
        &client
            .tools_call("host.usage", json!({}))
            .await
            .expect("host.usage"),
    );
    assert_eq!(
        usage["calls"].as_i64(),
        Some(1),
        "host.tool_call must record a calls row: {usage}"
    );

    // AC13: the invocation appears in host.tool_logs.
    let logs = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "hello"}))
            .await
            .expect("host.tool_logs"),
    );
    let lines = logs["lines"].as_array().expect("lines array");
    assert!(
        !lines.is_empty(),
        "host.tool_call's invocation must appear in host.tool_logs: {logs}"
    );
}

#[tokio::test]
async fn tool_call_rejects_invalid_args_like_the_direct_path() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Strict Caller").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("publish");

    let err = client
        .tools_call("host.tool_call", json!({"name": "hello", "args": {"nope": 1}}))
        .await
        .expect_err("args violating the kind's schema must fail");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
}

#[tokio::test]
async fn tool_call_on_another_tenants_name_is_not_found_without_a_leak() {
    let server = TestServer::start().await;
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    client_b
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("B publishes hello");

    let (_ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let err = client_a
        .tools_call("host.tool_call", json!({"name": "hello", "args": {}}))
        .await
        .expect_err("A never published hello");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
    let dump = format!("{err:?}");
    assert!(
        !dump.contains("Tenant B") && !dump.to_lowercase().contains("published"),
        "the error must not reveal that another tenant published this name: {dump}"
    );
}
