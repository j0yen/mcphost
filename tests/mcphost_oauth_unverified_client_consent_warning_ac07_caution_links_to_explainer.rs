//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC7 (P1) — Given the unverified caution renders, When the user reads it,
//! Then it links to a short explainer of what an unverified app means.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

async fn register_dcr_client(http: &reqwest::Client, base_url: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "web", "redirect_uris": ["https://consumer.example/callback"]}))
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

/// Pulls the `href` out of the first `<a href="...">` in `body` whose link
/// text is inside the caution block region -- simple substring scan is
/// enough since the consent page's own markup controls exactly one such
/// link inside `.caution`.
fn extract_explainer_href(body: &str) -> Option<String> {
    let caution_start = body.find("class=\"caution\"")?;
    let after = &body[caution_start..];
    let href_start = after.find("href=\"")? + "href=\"".len();
    let after_href = &after[href_start..];
    let href_end = after_href.find('"')?;
    Some(after_href[..href_end].to_string())
}

#[tokio::test]
async fn unverified_caution_links_to_an_explainer_that_resolves() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let client_id = register_dcr_client(&http, &server.base_url).await;

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id)).await.expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    let href = extract_explainer_href(&body).expect("caution block must contain a link: {body}");
    assert!(!href.is_empty(), "the explainer link must not be empty");

    let explainer_url = if href.starts_with("http") { href } else { format!("{}{}", server.base_url, href) };
    let explainer_resp = reqwest::get(&explainer_url).await.expect("GET the explainer link");
    assert_eq!(explainer_resp.status(), reqwest::StatusCode::OK, "the explainer link must resolve to a real page");
    let explainer_body = explainer_resp.text().await.expect("explainer body");
    assert!(
        explainer_body.to_lowercase().contains("unverified"),
        "the explainer page must actually explain what unverified means: {explainer_body}"
    );
}
