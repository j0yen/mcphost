//! PRD-mcphost-hosted-authorization-server
//! AC3 (P0) — Given `POST /oauth/register` with `application_type: native`
//! and `redirect_uris: ["http://127.0.0.1/cb"]`, When it runs, Then 201
//! with a `client_id`; When `/oauth/authorize` uses
//! `redirect_uri=http://127.0.0.1:53211/cb`, Then it is accepted; When the
//! same registration is attempted 11 times in a minute from one IP, Then
//! the 11th is 429.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn native_client_registers_and_loopback_port_is_accepted_at_authorize() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({
            "application_type": "native",
            "redirect_uris": ["http://127.0.0.1/cb"],
        }))
        .send()
        .await
        .expect("POST /oauth/register");
    assert_eq!(resp.status(), reqwest::StatusCode::CREATED);
    let body: Value = resp.json().await.expect("parse register response");
    let client_id = body["client_id"].as_str().expect("client_id present").to_string();
    assert!(!client_id.is_empty());

    let authorize_resp = client
        .get(format!(
            "{}/oauth/authorize?response_type=code&client_id={client_id}\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A53211%2Fcb\
             &code_challenge=dummy&code_challenge_method=S256&state=abc&scope=mcp\
             &resource={}%2Fmcp",
            server.base_url, server.base_url,
        ))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(
        authorize_resp.status(),
        reqwest::StatusCode::OK,
        "a registered loopback redirect must be accepted with any port"
    );
}

#[tokio::test]
async fn eleventh_registration_in_a_minute_from_one_ip_is_rate_limited() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();
    let payload = json!({
        "application_type": "native",
        "redirect_uris": ["http://127.0.0.1/cb"],
    });

    for i in 1..=10 {
        let resp = client
            .post(format!("{}/oauth/register", server.base_url))
            .json(&payload)
            .send()
            .await
            .expect("POST /oauth/register");
        assert_eq!(resp.status(), reqwest::StatusCode::CREATED, "registration {i} should succeed");
    }

    let eleventh = client
        .post(format!("{}/oauth/register", server.base_url))
        .json(&payload)
        .send()
        .await
        .expect("POST /oauth/register");
    assert_eq!(eleventh.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
}
