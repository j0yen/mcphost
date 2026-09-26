//! PRD-mcphost-end-user-identity
//! AC3 (P0) — Given an `end_user_assertion` signed with the wrong secret,
//! or one whose `exp` is already past, When a tool is called with it, Then
//! the call is rejected `end_user_assertion_invalid` and no tool executes.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::method;
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

fn sign_with(secret: &str, sub: &str, iat: i64, exp: i64) -> String {
    let claims = AssertionClaims { sub: sub.to_string(), iat, exp };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign assertion")
}

#[tokio::test]
async fn wrong_secret_and_past_exp_are_both_rejected_and_no_tool_runs() {
    let upstream = MockServer::start().await;
    // If the host called out anyway, this would match and return 200 --
    // the test asserts on `upstream.received_requests()` below to prove it
    // never did, for either rejected assertion.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let real_secret = common::extract_structured(&rotate)["secret"]
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

    // Wrong secret: a well-formed, unexpired token signed with a
    // different key entirely.
    let wrong_secret_token = sign_with("not-the-real-secret-at-all", "u3", now, now + 300);
    let err = client
        .tools_call(&format!("{ns}.whoami_call"), json!({"end_user_assertion": wrong_secret_token}))
        .await
        .expect_err("wrong-secret assertion must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("end_user_assertion_invalid"), "{err:?}");

    // Past exp: signed with the REAL secret, but already expired.
    let expired_token = sign_with(&real_secret, "u3", now - 7200, now - 3600);
    let err = client
        .tools_call(&format!("{ns}.whoami_call"), json!({"end_user_assertion": expired_token}))
        .await
        .expect_err("expired assertion must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("end_user_assertion_invalid"), "{err:?}");

    assert!(
        upstream.received_requests().await.unwrap().is_empty(),
        "no tool call may have reached the upstream for either rejected assertion"
    );

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let row = server
        .state
        .db
        .last_call_end_user_for_test(tenant.id, "whoami_call".to_string())
        .await
        .unwrap();
    assert!(row.is_none(), "no calls row may exist for whoami_call: {row:?}");
}
