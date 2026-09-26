//! PRD-mcphost-hosted-authorization-server
//! AC6 (P0) — Given `/oauth/authorize` without `code_challenge`, with
//! `code_challenge_method=plain`, with an unregistered `redirect_uri`,
//! with `resource=https://evil.test/mcp`, or with a tampered consent POST
//! lacking a valid key or claim code, When each runs, Then each is
//! refused (`invalid_request` / `invalid_target` / consent re-rendered)
//! and no code is minted; error responses redirect only to a registered
//! `redirect_uri` and otherwise render inline.

use crate::common;
use common::{TestServer, signup};
use serde_json::{Value, json};

const REDIRECT_URI: &str = "http://127.0.0.1/cb";

async fn register_client(server: &TestServer) -> String {
    let http = reqwest::Client::new();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": [REDIRECT_URI]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

#[tokio::test]
async fn missing_code_challenge_redirects_with_invalid_request() {
    let server = TestServer::start().await;
    let client_id = register_client(&server).await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let resp = http
        .get(format!(
            "{}/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb\
             &state=abc&scope=mcp&resource={}%2Fmcp",
            server.base_url, server.base_url,
        ))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert!(resp.status().is_redirection(), "status: {}", resp.status());
    let location = resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (base, query) = location.split_once('?').expect("query string");
    assert_eq!(base, REDIRECT_URI, "must redirect only to the registered redirect_uri");
    assert_eq!(query_param(query, "error"), Some("invalid_request"));
    assert_eq!(query_param(query, "state"), Some("abc"));
}

#[tokio::test]
async fn plain_code_challenge_method_redirects_with_invalid_request() {
    let server = TestServer::start().await;
    let client_id = register_client(&server).await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let resp = http
        .get(format!(
            "{}/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb\
             &code_challenge=dummy&code_challenge_method=plain&state=abc&scope=mcp&resource={}%2Fmcp",
            server.base_url, server.base_url,
        ))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert!(resp.status().is_redirection(), "status: {}", resp.status());
    let location = resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (base, query) = location.split_once('?').expect("query string");
    assert_eq!(base, REDIRECT_URI);
    assert_eq!(query_param(query, "error"), Some("invalid_request"));
}

#[tokio::test]
async fn unregistered_redirect_uri_renders_inline_with_no_redirect() {
    let server = TestServer::start().await;
    let client_id = register_client(&server).await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let resp = http
        .get(format!(
            "{}/oauth/authorize?response_type=code&client_id={client_id}\
             &redirect_uri=http%3A%2F%2Fattacker.example%2Fcb\
             &code_challenge=dummy&code_challenge_method=S256&state=abc&scope=mcp&resource={}%2Fmcp",
            server.base_url, server.base_url,
        ))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(resp.headers().get("location").is_none(), "must not redirect anywhere");
    let body = resp.text().await.unwrap();
    assert!(body.contains("invalid_request"), "body: {body}");
}

#[tokio::test]
async fn evil_resource_redirects_with_invalid_target() {
    let server = TestServer::start().await;
    let client_id = register_client(&server).await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let resp = http
        .get(format!(
            "{}/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb\
             &code_challenge=dummy&code_challenge_method=S256&state=abc&scope=mcp\
             &resource=https%3A%2F%2Fevil.test%2Fmcp",
            server.base_url,
        ))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert!(resp.status().is_redirection(), "status: {}", resp.status());
    let location = resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (base, query) = location.split_once('?').expect("query string");
    assert_eq!(base, REDIRECT_URI, "must redirect only to the registered redirect_uri");
    assert_eq!(query_param(query, "error"), Some("invalid_target"));
}

#[tokio::test]
async fn tampered_consent_post_with_no_valid_credential_re_renders_consent() {
    let server = TestServer::start().await;
    let (_ns, _key) = signup(&server.base_url, "Hardening Tenant").await;
    let client_id = register_client(&server).await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    let resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", "dummy-challenge"),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", "not-a-real-key-at-all"),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "must re-render, not redirect or error");
    assert!(resp.headers().get("location").is_none(), "must not redirect anywhere");
    let body = resp.text().await.unwrap();
    assert!(body.contains("form"), "should re-render the consent form: {body}");
}
