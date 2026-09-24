//! PRD-mcphost-human-claim-magic-link
//! AC3 (P0) — Given the verify URL from AC2, When it is opened once, Then
//! `tenants.owner_email` is `a@b.co`, `owner_verified_at` is set, the
//! summary page lists the tenant's published tool names and schedule
//! count, and opening the same verify URL again returns 410.

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
async fn verify_sets_owner_and_shows_summary_then_410s_on_reuse() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC3 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));
    let namespace = result["tenant"].as_str().expect("tenant").to_string();
    let key = result["key"].as_str().expect("key").to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    tenant_client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "pinger", "schedule": "0 0 1 1 *"}),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");

    let sends = fake.sends();
    assert_eq!(sends.len(), 1);
    let code = code_from_email_body(&sends[0].text_body).to_string();

    let verify_resp = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(verify_resp.status(), reqwest::StatusCode::OK);
    let body = verify_resp.text().await.expect("body");
    assert!(body.contains("pinger"), "summary must list the tool name: {body}");
    assert!(body.contains("Schedules: 1"), "summary must show the schedule count: {body}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert_eq!(tenant.owner_email.as_deref(), Some("a@b.co"));
    assert!(tenant.owner_verified_at.is_some());

    let second = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("second GET /claim/verify/{code}");
    assert_eq!(second.status(), reqwest::StatusCode::GONE);
}
