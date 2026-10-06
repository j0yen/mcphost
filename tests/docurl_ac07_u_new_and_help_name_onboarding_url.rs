//! PRD-mcphost-docs-one-url-flow
//! AC7 (P1) -- Given `/u/new` and `/help/tenant_key_missing`, When
//! fetched, Then both say no key is needed on `/mcp` and name
//! `onboarding.url`.

use crate::common::TestServer;

#[tokio::test]
async fn u_new_page_says_no_key_needed_on_mcp_and_names_onboarding_url() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/u/new", server.base_url))
        .send()
        .await
        .expect("GET /u/new");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("read body");

    assert!(
        body.contains("No key is needed on /mcp"),
        "/u/new must say no key is needed on /mcp: {body}"
    );
    assert!(
        body.contains("onboarding.url"),
        "/u/new must name onboarding.url: {body}"
    );
}

#[tokio::test]
async fn help_tenant_key_missing_says_no_key_needed_on_mcp_and_names_onboarding_url() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/help/tenant_key_missing", server.base_url))
        .send()
        .await
        .expect("GET /help/tenant_key_missing");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("read body");

    assert!(
        body.contains("no key is needed on the bare /mcp endpoint") || body.contains("no key is needed on /mcp"),
        "/help/tenant_key_missing must say no key is needed on /mcp: {body}"
    );
    assert!(
        body.contains("onboarding.url"),
        "/help/tenant_key_missing must name onboarding.url: {body}"
    );
}
