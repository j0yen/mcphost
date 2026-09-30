//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC6 (P1) — Given a `client_id` on the operator verified-client allowlist,
//! When its consent page renders, Then the unverified caution is suppressed
//! even though `method="dcr"`.

use crate::common;
use common::TestServer;

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
async fn allowlisted_dcr_client_id_renders_no_unverified_caution() {
    let client_id = "dcr_operator_verified_first_party_client".to_string();
    let server = TestServer::start_with_verified_client_ids(&[client_id.as_str()]).await;
    server
        .state
        .db
        .create_oauth_client(
            client_id.clone(),
            None,
            Some("First-Party mcphost Client".to_string()),
            serde_json::to_string(&vec!["https://consumer.example/callback"]).unwrap(),
            "web".to_string(),
            "none".to_string(),
        )
        .await
        .expect("seed operator-verified dcr client")
        .then_some(())
        .expect("client_id must not already exist");

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id)).await.expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    assert!(body.contains("First-Party mcphost Client"), "consent page should still name the client: {body}");
    assert!(
        !body.to_lowercase().contains("unverified"),
        "an operator-allowlisted DCR client's consent page must carry no unverified caution: {body}"
    );
    assert!(!body.contains("class=\"caution\""), "an allowlisted DCR client's consent page must render no caution block: {body}");
}

#[tokio::test]
async fn non_allowlisted_dcr_client_on_the_same_server_still_gets_the_caution() {
    // Guardrail: the allowlist must suppress the caution for its own
    // entries only -- a sibling DCR client on the very same server, not on
    // the list, is unaffected (the suppression isn't accidentally global).
    let allowlisted_id = "dcr_operator_verified_first_party_client_2".to_string();
    let server = TestServer::start_with_verified_client_ids(&[allowlisted_id.as_str()]).await;

    let other_id = "dcr_not_on_the_allowlist".to_string();
    server
        .state
        .db
        .create_oauth_client(
            other_id.clone(),
            None,
            Some("Some Other App".to_string()),
            serde_json::to_string(&vec!["https://consumer.example/callback"]).unwrap(),
            "web".to_string(),
            "none".to_string(),
        )
        .await
        .expect("seed non-allowlisted dcr client");

    let resp = reqwest::get(authorize_url(&server.base_url, &other_id)).await.expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.to_lowercase().contains("unverified"),
        "a non-allowlisted DCR client on the same server must still get the caution: {body}"
    );
}
