//! PRD-mcphost-end-user-identity
//! AC2 (P0) — Given a key-based call carrying a valid `end_user_assertion`
//! for `sub=u2`, When an `http`-kind tool runs, Then the upstream request
//! carries `X-MCPHost-End-User: u2`, and the `calls` row records
//! `end_user_method=assertion`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Serialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn assertion_reaches_http_headers_and_calls_row() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/whoami"))
        .and(header("X-MCPHost-End-User", "u2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"]
        .as_str()
        .expect("secret string")
        .to_string();

    let spec = json!({
        "method": "GET",
        "url": format!("{}/whoami", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "whoami_call", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let now = now_unix();
    let claims = AssertionClaims { sub: "u2".to_string(), iat: now, exp: now + 300 };
    let assertion = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign assertion");

    let result = client
        .tools_call(
            &format!("{ns}.whoami_call"),
            json!({"end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], 200, "{structured}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let (subject, issuer_col, method) = server
        .state
        .db
        .last_call_end_user_for_test(tenant.id, "whoami_call".to_string())
        .await
        .unwrap()
        .expect("a calls row must exist for whoami_call");
    assert_eq!(subject.as_deref(), Some("u2"), "calls.end_user_subject");
    assert_eq!(issuer_col, None, "an assertion carries no issuer: calls.end_user_issuer");
    assert_eq!(method.as_deref(), Some("assertion"), "calls.end_user_method");
}
