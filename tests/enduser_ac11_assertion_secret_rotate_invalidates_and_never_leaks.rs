//! PRD-mcphost-end-user-identity
//! AC11 (P0) — Given `host.enduser.assertion_secret_rotate` called twice,
//! When an assertion signed with the first secret is presented after the
//! second call, Then it is rejected `end_user_assertion_invalid`; the
//! secret value never appears in `host.secret_list` output.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
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

fn sign_with(secret: &str, sub: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign assertion")
}

#[tokio::test]
async fn second_rotation_invalidates_the_first_secret_and_never_leaks() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC11 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

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

    let rotate1 = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("first rotate ok");
    let secret1 = extract_structured(&rotate1)["secret"].as_str().expect("secret1").to_string();

    // The first secret works before any second rotation.
    let token_secret1 = sign_with(&secret1, "u1");
    client
        .tools_call(&format!("{ns}.whoami_call"), json!({"end_user_assertion": token_secret1}))
        .await
        .expect("assertion signed with secret1 must be accepted before rotation");

    let rotate2 = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("second rotate ok");
    let secret2 = extract_structured(&rotate2)["secret"].as_str().expect("secret2").to_string();
    assert_ne!(secret1, secret2, "each rotation must generate a fresh secret");

    // A fresh token signed with the now-stale secret1 is rejected, no
    // overlap window.
    let stale_token = sign_with(&secret1, "u1");
    let err = client
        .tools_call(&format!("{ns}.whoami_call"), json!({"end_user_assertion": stale_token}))
        .await
        .expect_err("an assertion signed with the first secret must be rejected after rotation");
    assert_eq!(err.error_code.as_deref(), Some("end_user_assertion_invalid"), "{err:?}");

    // A token signed with the current secret still works.
    let fresh_token = sign_with(&secret2, "u1");
    client
        .tools_call(&format!("{ns}.whoami_call"), json!({"end_user_assertion": fresh_token}))
        .await
        .expect("assertion signed with the current secret must be accepted");

    // Neither secret value ever appears in host.secret_list output --
    // that endpoint returns only names, never values, for any secret.
    let listed = client
        .tools_call("host.secret_list", json!({}))
        .await
        .expect("secret_list ok");
    let dump = listed.to_string();
    assert!(!dump.contains(&secret1), "secret1 leaked into host.secret_list: {dump}");
    assert!(!dump.contains(&secret2), "secret2 leaked into host.secret_list: {dump}");

    // A tenant may never see or set the reserved name's value directly --
    // the guard that keeps host.secret_set from letting a tenant choose
    // (and therefore know) its own assertion secret.
    let err = client
        .tools_call(
            "host.secret_set",
            json!({"name": "__mcphost_end_user_assertion__", "value": "attacker-chosen"}),
        )
        .await
        .expect_err("a tenant must not be able to set the reserved assertion secret name directly");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
}
