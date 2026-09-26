//! PRD-mcphost-upstream-token-vault
//! AC7 (P0) — Given a used handoff token, When the connect URL is opened
//! again, Then it returns 410 and no second authorization starts.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::MockServer;

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

async fn sign_assertion(client: &McpClient, subject: &str) -> String {
    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"]
        .as_str()
        .expect("secret string")
        .to_string();
    let now = now_unix();
    let claims = AssertionClaims { sub: subject.to_string(), iat: now, exp: now + 300 };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign assertion")
}

#[tokio::test]
async fn second_open_of_a_used_connect_link_is_gone() {
    let oauth_server = MockServer::start().await;

    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": format!("{}/authorize", oauth_server.uri()),
                "token_url": format!("{}/token", oauth_server.uri()),
                "client_id": "cid-123",
                "client_secret": "shh-secret",
                "scopes": ["read"],
            }),
        )
        .await
        .expect("provider_set ok");

    let assertion = sign_assertion(&client, "u1").await;
    let result = client
        .tools_call(
            "host.vault.connect_link",
            json!({"provider": "slack", "end_user": "self", "end_user_assertion": assertion}),
        )
        .await
        .expect("connect_link ok");
    let connect_link = extract_structured(&result)["connect_link"]
        .as_str()
        .expect("connect_link string")
        .to_string();

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build no-redirect client");

    let first = http.get(&connect_link).send().await.expect("first GET");
    assert_eq!(first.status(), reqwest::StatusCode::FOUND, "first open must redirect");

    let second = http.get(&connect_link).send().await.expect("second GET");
    assert_eq!(second.status(), reqwest::StatusCode::GONE, "a reused connect link must 410");
    assert!(
        second.headers().get(reqwest::header::LOCATION).is_none(),
        "a 410 must never carry a redirect Location -- no second authorization starts"
    );

    assert!(
        oauth_server.received_requests().await.unwrap().is_empty(),
        "this test only ever inspects the redirect Location -- the provider itself is never actually reached"
    );
}
