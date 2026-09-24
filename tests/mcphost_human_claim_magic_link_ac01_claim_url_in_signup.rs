//! PRD-mcphost-human-claim-magic-link
//! AC1 (P0) — Given an unauthenticated `signup` (key mode or handoff
//! mode), When it succeeds, Then the response contains `claim_url`
//! matching `^https://[^/]+/claim/[A-Za-z0-9_-]{40,}$` and the token is
//! not the bearer key.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

/// No regex crate dependency needed for one fixed shape: `https://`, a
/// host with no `/`, then `/claim/` and 40+ URL-safe characters, nothing
/// else after.
fn matches_claim_url_shape(url: &str) -> bool {
    let Some(after_scheme) = url.strip_prefix("https://") else {
        return false;
    };
    let Some(slash) = after_scheme.find('/') else {
        return false;
    };
    let host = &after_scheme[..slash];
    let rest = &after_scheme[slash..];
    let Some(token) = rest.strip_prefix("/claim/") else {
        return false;
    };
    !host.is_empty()
        && token.len() >= 40
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[tokio::test]
async fn key_mode_signup_returns_claim_url() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "Claim Key Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);

    let claim_url = result["claim_url"].as_str().expect("claim_url field");
    assert!(
        matches_claim_url_shape(claim_url),
        "claim_url '{claim_url}' does not match the required shape"
    );
    let key = result["key"].as_str().expect("key field");
    assert!(
        !claim_url.contains(key),
        "claim_url must not embed the bearer key"
    );
}

#[tokio::test]
async fn handoff_mode_signup_returns_claim_url() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "Claim Handoff Tenant", "handoff": true}))
        .await
        .expect("signup(handoff: true)");
    let result = extract_structured(&raw);

    let claim_url = result["claim_url"].as_str().expect("claim_url field");
    assert!(
        matches_claim_url_shape(claim_url),
        "claim_url '{claim_url}' does not match the required shape"
    );
    let handoff_token = result["handoff_token"]
        .as_str()
        .expect("handoff_token field");
    assert!(
        !claim_url.contains(handoff_token),
        "claim_url must not embed the handoff token either"
    );
    assert!(result.get("key").is_none(), "handoff mode must not carry a raw key");
}
