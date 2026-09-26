//! PRD-mcphost-hosted-authorization-server
//! AC2 (P0) — Given a test HTTPS server hosting a CIMD document at
//! `https://client.test/cimd.json` with one redirect URI, When
//! `/oauth/authorize` is called with `client_id` equal to that URL and
//! that redirect, Then the consent page renders naming the client; When
//! called with a different redirect or the document's `client_id`
//! mismatches the URL, Then `invalid_request` rendered inline with no
//! redirect.

use crate::common;
use common::TestServer;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REDIRECT_URI: &str = "https://client.example/callback";

async fn cimd_server_with_doc(client_id_in_doc_matches_url: bool) -> (MockServer, String) {
    let server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", server.uri());
    let doc_client_id = if client_id_in_doc_matches_url {
        cimd_url.clone()
    } else {
        "https://not-the-same-url.example/cimd.json".to_string()
    };
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": doc_client_id,
            "client_name": "Test Connector",
            "redirect_uris": [REDIRECT_URI],
        })))
        .mount(&server)
        .await;
    (server, cimd_url)
}

fn authorize_url(base_url: &str, client_id: &str, redirect_uri: &str) -> String {
    format!(
        "{base_url}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}\
         &code_challenge=dummy-challenge&code_challenge_method=S256&state=abc&scope=mcp&resource={base_url}/mcp",
        urlencoding_stub(client_id),
        urlencoding_stub(redirect_uri),
    )
}

/// No `urlencoding` dependency in this crate -- percent-encodes just the
/// handful of characters these test URLs actually contain (`:`, `/`).
fn urlencoding_stub(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

#[tokio::test]
async fn matching_client_id_and_redirect_renders_consent_naming_the_client() {
    let server = TestServer::start().await;
    let (_cimd, cimd_url) = cimd_server_with_doc(true).await;

    let resp = reqwest::get(authorize_url(&server.base_url, &cimd_url, REDIRECT_URI))
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("Test Connector"), "consent page should name the client: {body}");
}

#[tokio::test]
async fn different_redirect_uri_is_invalid_request_inline_with_no_redirect() {
    let server = TestServer::start().await;
    let (_cimd, cimd_url) = cimd_server_with_doc(true).await;

    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(authorize_url(&server.base_url, &cimd_url, "https://attacker.example/cb"))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(resp.headers().get("location").is_none(), "must not redirect anywhere");
    let body = resp.text().await.expect("body");
    assert!(body.contains("invalid_request"), "body: {body}");
}

#[tokio::test]
async fn cimd_document_client_id_mismatch_is_invalid_request_inline_with_no_redirect() {
    let server = TestServer::start().await;
    let (_cimd, cimd_url) = cimd_server_with_doc(false).await;

    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(authorize_url(&server.base_url, &cimd_url, REDIRECT_URI))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(resp.headers().get("location").is_none(), "must not redirect anywhere");
    let body = resp.text().await.expect("body");
    assert!(body.contains("invalid_request"), "body: {body}");
}
