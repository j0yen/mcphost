//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC1 (P0) — Given a DCR-registered client (`method="dcr"`) that is not
//! operator-verified, When a user reaches its `/oauth/authorize` consent
//! page, Then the page renders an "unverified application" caution block
//! stating the app self-registered and is not verified by mcphost.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

async fn register_dcr_client(http: &reqwest::Client, base_url: &str, client_name: Option<&str>) -> String {
    let mut body = json!({"application_type": "web", "redirect_uris": ["https://consumer.example/callback"]});
    if let Some(name) = client_name {
        body["client_name"] = json!(name);
    }
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&body)
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
async fn dcr_non_allowlisted_client_consent_page_carries_unverified_caution() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let client_id = register_dcr_client(&http, &server.base_url, Some("Some DCR App")).await;

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id, "https://consumer.example/callback"))
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    assert!(body.contains("caution"), "consent page must carry the caution block: {body}");
    assert!(
        body.to_lowercase().contains("unverified"),
        "consent page must call out the client as unverified: {body}"
    );
    assert!(
        body.contains("registered itself") || body.contains("self-registered"),
        "caution must state the app self-registered: {body}"
    );
    assert!(
        body.contains("not been verified") || body.contains("not verified"),
        "caution must state mcphost has not verified the app: {body}"
    );
}
