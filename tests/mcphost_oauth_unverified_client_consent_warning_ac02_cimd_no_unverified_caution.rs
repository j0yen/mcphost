//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC2 (P0) — Given a CIMD client (`method="cimd"`), When a user reaches
//! its consent page, Then no unverified-application caution block is
//! rendered.

use crate::common;
use common::TestServer;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REDIRECT_URI: &str = "https://client.example/callback";

async fn cimd_server_with_doc() -> (MockServer, String) {
    let server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", server.uri());
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_url,
            "client_name": "Test Connector",
            "redirect_uris": [REDIRECT_URI],
        })))
        .mount(&server)
        .await;
    (server, cimd_url)
}

fn authorize_url(base_url: &str, client_id: &str, redirect_uri: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("{base_url}/oauth/authorize")).expect("parse base authorize url");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", "dummy-challenge")
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", "abc")
        .append_pair("scope", "mcp")
        .append_pair("resource", &format!("{base_url}/mcp"));
    url.to_string()
}

#[tokio::test]
async fn cimd_client_consent_page_carries_no_unverified_caution() {
    let server = TestServer::start().await;
    let (_cimd, cimd_url) = cimd_server_with_doc().await;

    let resp = reqwest::get(authorize_url(&server.base_url, &cimd_url, REDIRECT_URI))
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    assert!(body.contains("Test Connector"), "consent page should still name the client: {body}");
    assert!(
        !body.to_lowercase().contains("unverified"),
        "a CIMD client's consent page must carry no unverified caution: {body}"
    );
    assert!(
        !body.contains("class=\"caution\""),
        "a CIMD client's consent page must render no caution block: {body}"
    );
}
