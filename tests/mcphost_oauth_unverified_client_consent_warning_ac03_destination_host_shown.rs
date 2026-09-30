//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC3 (P0) — Given a consent page for any third-party client, When it
//! renders, Then it displays the destination host (the host of the
//! `redirect_uri` the authorization code would be delivered to).

use crate::common;
use common::TestServer;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn register_dcr_client(http: &reqwest::Client, base_url: &str, redirect_uri: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "web", "redirect_uris": [redirect_uri]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
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
async fn dcr_client_consent_page_shows_redirect_destination_host() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let redirect_uri = "https://consumer.example/callback";
    let client_id = register_dcr_client(&http, &server.base_url, redirect_uri).await;

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id, redirect_uri))
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("consumer.example"),
        "consent page must show the redirect_uri's destination host: {body}"
    );
}

#[tokio::test]
async fn cimd_client_consent_page_also_shows_redirect_destination_host() {
    let server = TestServer::start().await;
    let cimd_server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", cimd_server.uri());
    let redirect_uri = "https://cimd-consumer.example/callback";
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_url,
            "client_name": "CIMD Connector",
            "redirect_uris": [redirect_uri],
        })))
        .mount(&cimd_server)
        .await;

    let resp = reqwest::get(authorize_url(&server.base_url, &cimd_url, redirect_uri))
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("cimd-consumer.example"),
        "a CIMD client's consent page must also show the destination host: {body}"
    );
}
