//! AC1 — Given a tenant with secret `stripe` set, When it publishes an
//! `http` tool with `Authorization: Bearer {{secret.stripe}}` and calls it
//! against a stub upstream, Then the upstream receives the secret in the
//! header, the tool result contains the upstream JSON, and the secret value
//! appears in no log line, result or error.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn upstream_receives_secret_and_result_has_it_redacted() {
    let upstream = MockServer::start().await;
    // The matcher itself proves the real secret value reached the upstream
    // header: a request without it 404s and the tool call fails.
    Mock::given(method("GET"))
        .and(path("/v1/customers/cus_123"))
        .and(header(
            "Authorization",
            "Bearer sk_live_canary_secret_value",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "cus_123",
            "echo_secret": "sk_live_canary_secret_value",
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Stripe Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "stripe", "value": "sk_live_canary_secret_value"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/customers/{{{{id}}}}", upstream.uri()),
        "headers": {"Authorization": "Bearer {{secret.stripe}}"},
        "args_schema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "stripe_customer_get", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // PRD-mcphost-secret-non-exfiltration-proof R2: the stub upstream
    // deliberately echoes the secret back, which used to be redacted to
    // `***` and returned ok; it is now a block. The matcher above still
    // proves the upstream received the real value (a request without it
    // 404s, which would surface as `upstream_status`, not as the block).
    let err = client
        .tools_call(
            &format!("{ns}.stripe_customer_get"),
            json!({"id": "cus_123"}),
        )
        .await
        .expect_err("an upstream that echoes the secret must be blocked");
    assert_eq!(err.error_code.as_deref(), Some("secret_leak_blocked"), "{err:?}");
    assert_eq!(err.data["where"], "result");

    // Guardrail: the secret value appears nowhere in the error.
    let dump = format!("{} {}", err.message, err.data);
    assert!(
        !dump.contains("sk_live_canary_secret_value"),
        "secret value leaked into the error: {dump}"
    );

    // Guardrail: the secret never appears in `host.tool_logs` either.
    let logs = client
        .tools_call("host.tool_logs", json!({"name": "stripe_customer_get"}))
        .await
        .expect("tool_logs ok");
    let logs_dump = extract_structured(&logs).to_string();
    assert!(
        !logs_dump.contains("sk_live_canary_secret_value"),
        "secret value leaked into tool_logs: {logs_dump}"
    );
}
