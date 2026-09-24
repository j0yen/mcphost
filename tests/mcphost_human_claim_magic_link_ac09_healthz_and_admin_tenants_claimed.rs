//! PRD-mcphost-human-claim-magic-link
//! AC9 (P0) — Given a claimed tenant, When the operator calls admin
//! healthz, Then `tenants_claimed.external` counts it and `admin.tenants`
//! shows `owner_verified: true` without an email address in the row.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
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

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn claimed_tenant_counts_in_healthz_and_admin_tenants() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;

    let before = healthz(&server.base_url).await;
    let claimed_external_before = before["tenants_claimed"]["external"].as_i64().unwrap();

    // Same "call control::signup directly with a fake external IP" seam
    // tests/attrib_ac4_external_signup_counts_real.rs uses -- a real
    // connection to this TestServer's listener is always 127.0.0.1
    // (classified `synthetic`/loopback), so an `origin = 'external'`
    // tenant for this AC's own `tenants_claimed.external` assertion has to
    // be minted at the business-logic layer instead.
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC9 Tenant"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let namespace = result["tenant"].as_str().expect("tenant").to_string();
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "ac9@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let code = code_from_email_body(&fake.sends()[0].text_body).to_string();
    let verify_resp = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(verify_resp.status(), reqwest::StatusCode::OK);

    let after = healthz(&server.base_url).await;
    let claimed_external_after = after["tenants_claimed"]["external"].as_i64().unwrap();
    assert_eq!(claimed_external_after, claimed_external_before + 1);

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listing = extract_structured(
        &admin
            .tools_call("admin.tenants", json!({}))
            .await
            .expect("admin.tenants"),
    );
    let row = listing["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .find(|t| t["tenant"] == json!(namespace))
        .expect("this tenant's row")
        .clone();
    assert_eq!(row["owner_verified"], json!(true));
    assert!(row.get("owner_email").is_none(), "row: {row}");
    let row_text = row.to_string();
    assert!(
        !row_text.contains("ac9@example.com"),
        "the owner's email must never appear in admin.tenants: {row_text}"
    );
}
