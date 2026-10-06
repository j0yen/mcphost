//! PRD-mcphost-reachability-alt-host
//! AC5 (P0) — Given no `MCPHOST_ALT_PUBLIC_URLS`, When links are rendered,
//! Then no `alt:` line appears and behaviour matches today: the claim
//! email, `host.quickstart` and the invite response carry none of
//! `alt_url`/`alt_endpoint`/`reachability`, not even present-but-empty.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn no_alternates_configured_means_no_fallback_fields_anywhere() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "AC5 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    assert!(result.get("alt_endpoint").is_none());
    assert!(result.get("alt_claim_url").is_none());
    assert!(result.get("reachability").is_none());

    let claim_url = result["claim_url"].as_str().expect("claim_url");
    let token = token_from_claim_url(claim_url);
    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}")
        .error_for_status()
        .expect("200");
    assert_eq!(fake.send_count(), 1);
    let body = &fake.sends()[0].text_body;
    assert!(!body.contains("alt:"), "no alternates configured must mean no alt: line: {body}");
    assert!(!body.contains("/cant-reach"), "no allow-steps line either: {body}");

    let key = result["key"].as_str().expect("key");
    let authed = McpClient::with_bearer(&server.base_url, key);
    let quickstart = extract_structured(
        &authed
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("host.quickstart"),
    );
    assert!(quickstart.get("alt_endpoint").is_none());
    assert!(quickstart.get("reachability").is_none());
    assert_eq!(
        quickstart["endpoint"],
        json!(format!("{}/mcp", server.base_url)),
        "endpoint itself is additive and present either way"
    );

    let invite = extract_structured(
        &authed.tools_call("host.invite.create", json!({})).await.expect("host.invite.create"),
    );
    assert!(invite.get("alt_url").is_none());
}
