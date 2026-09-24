//! PRD-mcphost-human-claim-magic-link
//! AC11 (P1) — Given a claimed tenant, When the agent calls
//! `host.whoami`, Then `owner_verified` is true; for an unclaimed tenant
//! it is false.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after.split_whitespace().next().expect("code token")
}

#[tokio::test]
async fn whoami_reports_owner_verified_true_only_after_claim() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC11 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let key = result["key"].as_str().expect("key").to_string();
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let before = extract_structured(
        &tenant_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami before claim"),
    );
    assert_eq!(before["owner_verified"], json!(false));

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let code = code_from_email_body(&fake.sends()[0].text_body).to_string();
    http.get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");

    let after = extract_structured(
        &tenant_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami after claim"),
    );
    assert_eq!(after["owner_verified"], json!(true));
}
