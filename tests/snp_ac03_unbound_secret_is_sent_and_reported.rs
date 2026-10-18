//! PRD mcphost-secret-non-exfiltration-proof AC3 — Given an unbound secret sent to any host, When called, Then the
//! request is sent, the result succeeds, and `_meta.unbound_secret_sent ==
//! ["x"]`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "sk_live_canary_secret_value";

async fn call_with_secret(hosts: Option<Value>) -> (Value, usize) {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("Authorization", format!("Bearer {SECRET}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Unbound Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut set = json!({"name": "x", "value": SECRET});
    if let Some(hosts) = hosts {
        set["hosts"] = hosts;
    }
    client.tools_call("host.secret.set", set).await.expect("secret set ok");
    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/ping", upstream.uri()),
        "headers": {"Authorization": "Bearer {{ secret.x }}"},
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "ping", "kind": "http", "spec": spec}))
        .await
        .expect("publish ok");
    let result = client
        .tools_call(&format!("{ns}.ping"), json!({}))
        .await
        .expect("the call must succeed");
    let sent = upstream.received_requests().await.unwrap_or_default().len();
    (result, sent)
}

#[tokio::test]
async fn unbound_secret_goes_out_and_is_reported_in_meta() {
    let (result, sent) = call_with_secret(None).await;
    assert_eq!(sent, 1, "the request must have been sent");
    assert_eq!(extract_structured(&result)["status"], 200);
    assert_eq!(result["_meta"]["unbound_secret_sent"], json!(["x"]), "{result}");
}

#[tokio::test]
async fn bound_secret_to_its_host_is_not_reported() {
    let (result, sent) = call_with_secret(Some(json!(["127.0.0.1"]))).await;
    assert_eq!(sent, 1);
    assert_eq!(extract_structured(&result)["status"], 200);
    assert!(
        result.get("_meta").and_then(|m| m.get("unbound_secret_sent")).is_none(),
        "a bound secret sent to its own host must not be reported: {result}"
    );
}
