//! PRD-mcphost-human-claim-magic-link
//! AC7 (P0) — Given two verify requests for the same tenant racing (two
//! different codes issued to two emails), When both complete, Then
//! exactly one owner is set and the loser receives 409.

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
async fn racing_verify_codes_have_exactly_one_winner() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC7 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));
    let namespace = result["tenant"].as_str().expect("tenant").to_string();

    let http = reqwest::Client::new();
    // Two magic-link sends, two different emails, both against the same
    // still-unclaimed tenant.
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "first@example.com")])
        .send()
        .await
        .expect("first POST /claim/{token}");
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "second@example.com")])
        .send()
        .await
        .expect("second POST /claim/{token}");

    let sends = fake.sends();
    assert_eq!(sends.len(), 2);
    let code_a = code_from_email_body(&sends[0].text_body).to_string();
    let code_b = code_from_email_body(&sends[1].text_body).to_string();

    let base = server.base_url.clone();
    let (resp_a, resp_b) = tokio::join!(
        http.get(format!("{base}/claim/verify/{code_a}")).send(),
        http.get(format!("{base}/claim/verify/{code_b}")).send(),
    );
    let status_a = resp_a.expect("verify a").status();
    let status_b = resp_b.expect("verify b").status();

    let statuses = [status_a, status_b];
    let ok_count = statuses.iter().filter(|s| **s == reqwest::StatusCode::OK).count();
    let conflict_count = statuses
        .iter()
        .filter(|s| **s == reqwest::StatusCode::CONFLICT)
        .count();
    assert_eq!(ok_count, 1, "exactly one verify must win: {statuses:?}");
    assert_eq!(conflict_count, 1, "exactly one verify must lose with 409: {statuses:?}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert!(tenant.owner_verified_at.is_some());
    assert!(
        tenant.owner_email.as_deref() == Some("first@example.com")
            || tenant.owner_email.as_deref() == Some("second@example.com"),
        "owner_email must be exactly one of the two racing emails: {:?}",
        tenant.owner_email
    );
}
