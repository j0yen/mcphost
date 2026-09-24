//! PRD-mcphost-human-claim-magic-link
//! AC13 (P0) — Given no page in www/ or any template, When grepped for
//! the bearer key of a test tenant after a full claim flow, Then zero
//! occurrences in any rendered response body.

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
async fn bearer_key_never_appears_in_any_claim_page_body() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC13 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let key = result["key"].as_str().expect("key").to_string();
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let http = reqwest::Client::new();
    let mut bodies = Vec::new();

    let form_page = http
        .get(format!("{}/claim/{claim_token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token}")
        .text()
        .await
        .expect("body");
    bodies.push(("GET /claim/{token}", form_page));

    let submit_page = http
        .post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}")
        .text()
        .await
        .expect("body");
    bodies.push(("POST /claim/{token}", submit_page));

    let code = code_from_email_body(&fake.sends()[0].text_body).to_string();
    let summary_page = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}")
        .text()
        .await
        .expect("body");
    bodies.push(("GET /claim/verify/{code}", summary_page));

    for (label, body) in &bodies {
        assert!(
            !body.contains(&key),
            "{label}'s response body must never contain the bearer key: {body}"
        );
    }
}
