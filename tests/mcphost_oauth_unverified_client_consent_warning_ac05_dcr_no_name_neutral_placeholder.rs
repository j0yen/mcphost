//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC5 (P0) — Given a DCR client that registered with no `client_name`,
//! When the consent page renders, Then it shows a neutral placeholder and
//! still renders the unverified caution and the destination host, with no
//! crash and no empty-name injection.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

async fn register_dcr_client_without_name(http: &reqwest::Client, base_url: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({
            "application_type": "web",
            "redirect_uris": ["https://consumer.example/callback"],
        }))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

fn authorize_url(base_url: &str, client_id: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("{base_url}/oauth/authorize")).expect("parse base authorize url");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", "https://consumer.example/callback")
        .append_pair("code_challenge", "dummy-challenge")
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", "abc")
        .append_pair("scope", "mcp")
        .append_pair("resource", &format!("{base_url}/mcp"));
    url.to_string()
}

#[tokio::test]
async fn nameless_dcr_client_renders_placeholder_caution_and_destination_without_crashing() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let client_id = register_dcr_client_without_name(&http, &server.base_url).await;

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id)).await.expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "a nameless client must not crash the consent render");
    let body = resp.text().await.expect("body");

    assert!(!body.contains("<h1>Authorize </h1>"), "must never render an empty client name: {body}");
    assert!(
        body.contains("(unnamed application)"),
        "must show a neutral placeholder in place of the missing name: {body}"
    );
    assert!(
        body.to_lowercase().contains("unverified"),
        "the unverified caution must still render for a nameless DCR client: {body}"
    );
    assert!(
        body.contains("consumer.example"),
        "the destination host must still render for a nameless DCR client: {body}"
    );
}
