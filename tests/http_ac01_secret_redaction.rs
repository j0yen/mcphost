//! AC1 — Given a tenant with secret `stripe` set, When it publishes an
//! `http` tool with `Authorization: Bearer {{secret.stripe}}` and calls it
//! against a stub upstream, Then the upstream receives the secret in the
//! header, the tool result contains the upstream JSON, and the secret value
//! appears in no log line, result or error.

mod common;
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

    let result = client
        .tools_call(
            &format!("{ns}.stripe_customer_get"),
            json!({"id": "cus_123"}),
        )
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    // The upstream really did receive the secret (the matcher above would
    // have 404'd otherwise, which would show up as an `upstream_status`
    // error rather than success) -- so this is a real end-to-end proof, not
    // an assumption.
    assert_eq!(structured["status"], 200);
    assert_eq!(structured["body"]["id"], "cus_123");

    // Guardrail: the secret value appears nowhere in the tool's own result,
    // even though the stub upstream deliberately echoed it back.
    let dump = structured.to_string();
    assert!(
        !dump.contains("sk_live_canary_secret_value"),
        "secret value leaked into the tool result: {dump}"
    );
    assert!(
        dump.contains("***"),
        "expected the echoed secret to be redacted to ***: {dump}"
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
