//! PRD-mcphost-human-claim-magic-link
//! AC8 (P0) — Given 31 `GET /claim/*` requests from one IP within an hour
//! at the default limit, When the 31st arrives, Then 429 and the tenant
//! row is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn the_31st_claim_request_from_one_ip_in_an_hour_is_429() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC8 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();
    let namespace = result["tenant"].as_str().expect("tenant").to_string();

    let http = reqwest::Client::new();
    let mut statuses = Vec::new();
    for _ in 0..31 {
        let resp = http
            .get(format!("{}/claim/{token}", server.base_url))
            .send()
            .await
            .expect("GET /claim/{token}");
        statuses.push(resp.status());
    }

    let too_many = statuses
        .iter()
        .filter(|s| **s == reqwest::StatusCode::TOO_MANY_REQUESTS)
        .count();
    assert_eq!(too_many, 1, "exactly the 31st request should be rate-limited: {statuses:?}");
    assert_eq!(
        statuses[30],
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "the 31st request specifically must be 429: {statuses:?}"
    );
    for s in &statuses[..30] {
        assert_ne!(*s, reqwest::StatusCode::TOO_MANY_REQUESTS);
    }

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert!(tenant.owner_email.is_none());
    assert!(tenant.owner_verified_at.is_none());
}
