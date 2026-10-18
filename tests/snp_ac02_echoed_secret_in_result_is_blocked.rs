//! PRD mcphost-secret-non-exfiltration-proof AC2 — Given a mock upstream that echoes its request body, and an http
//! tool posting `{{ secret.x }}` to a bound host, When called, Then the
//! result is `secret_leak_blocked {where: "result"}`, the `calls` row has
//! `error_class = secret_leak_blocked`, and the tool log line is redacted.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SECRET: &str = "sk_live_canary_secret_value";

/// Answers every request with its own body, as JSON.
pub struct EchoBody;

impl Respond for EchoBody {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_bytes(request.body.clone())
    }
}

#[tokio::test]
async fn echoed_secret_fails_the_call_and_the_log_stays_redacted() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST")).respond_with(EchoBody).mount(&upstream).await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Leak Result Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.secret.set",
            json!({"name": "x", "value": SECRET, "hosts": ["127.0.0.1"]}),
        )
        .await
        .expect("secret set ok");
    let spec = json!({
        "method": "POST",
        "url": format!("{}/echo", upstream.uri()),
        "body": {"token": "{{ secret.x }}"},
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "echo", "kind": "http", "spec": spec}))
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.echo"), json!({}))
        .await
        .expect_err("an echoed secret must fail the call");
    assert_eq!(err.error_code.as_deref(), Some("secret_leak_blocked"), "{err:?}");
    assert_eq!(err.data["where"], "result");
    assert_eq!(err.data["secret"], "x");
    assert!(!err.data.to_string().contains(SECRET) && !err.message.contains(SECRET), "{err:?}");
    // the request itself went out: the bound host is the mock's own
    assert_eq!(upstream.received_requests().await.unwrap_or_default().len(), 1);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db ok")
        .expect("tenant exists");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let class: String = conn
        .query_row(
            "SELECT error_class FROM calls WHERE tenant_id = ?1 ORDER BY id DESC LIMIT 1",
            [tenant.id],
            |r| r.get(0),
        )
        .expect("the blocked call's row");
    assert_eq!(class, "secret_leak_blocked");

    let logs = client
        .tools_call("host.tool_logs", json!({"name": "echo"}))
        .await
        .expect("tool_logs ok");
    let dump = extract_structured(&logs).to_string();
    assert!(!dump.contains(SECRET), "secret value leaked into tool_logs: {dump}");
    assert!(dump.contains("secret_leak_blocked"), "the block is not in the log: {dump}");
}
