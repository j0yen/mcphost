//! PRD mcphost-secret-non-exfiltration-proof AC6 — Given a secret value shorter than 8 characters, When it appears as
//! a whole token in a result, Then it is blocked; When it appears inside a
//! longer word, Then the call succeeds (no false block).

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn setup() -> (MockServer, TestServer, McpClient, String) {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/whole"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"msg": "the key is abc, sorry"})))
        .mount(&upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/word"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"msg": "abcdef and xabc and abc_1"})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Short Secret Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.secret.set", json!({"name": "x", "value": "abc", "hosts": ["127.0.0.1"]}))
        .await
        .expect("secret set ok");
    let spec = json!({
        "method": "GET",
        "url": format!("{}/{{{{ p }}}}", upstream.uri()),
        "headers": {"X-Key": "{{ secret.x }}"},
        "args_schema": {"type": "object", "properties": {"p": {"type": "string"}}, "required": ["p"]},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "peek", "kind": "http", "spec": spec}))
        .await
        .expect("publish ok");
    (upstream, server, client, ns)
}

#[tokio::test]
async fn a_short_secret_as_a_whole_token_is_blocked() {
    let (_upstream, _server, client, ns) = setup().await;
    let err = client
        .tools_call(&format!("{ns}.peek"), json!({"p": "whole"}))
        .await
        .expect_err("the whole-token echo must be blocked");
    assert_eq!(err.error_code.as_deref(), Some("secret_leak_blocked"), "{err:?}");
    assert_eq!(err.data["where"], "result");
}

#[tokio::test]
async fn a_short_secret_inside_a_longer_word_is_not_a_false_block() {
    let (_upstream, _server, client, ns) = setup().await;
    let result = client
        .tools_call(&format!("{ns}.peek"), json!({"p": "word"}))
        .await
        .expect("a short secret inside longer words must not block");
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], 200);
    // `abc_1` is one token too (`_` is a word character), so nothing is redacted either
    assert_eq!(structured["body"]["msg"], "abcdef and xabc and abc_1", "{structured}");
}
