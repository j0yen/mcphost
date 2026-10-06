//! PRD-mcphost-ownership-moment
//! AC4 (P0) — Given three external claims and one synthetic claim in the
//! window, When `/healthz` is read, Then `claims.external = 3`,
//! `claims.synthetic = 1`, and `median_minutes_to_claim` is computed from
//! `owner_verified_at - created_unix`.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use serde_json::json;

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

/// Signs up a tenant (any source works -- `origin` is overwritten right
/// after by [`set_claim_timing_for_test`]) and back-dates its claim timing
/// so `median_minutes_to_claim` lands on an exact, test-chosen value with
/// no real wait.
async fn claimed_tenant(server: &TestServer, origin: &str, minutes_to_claim: i64, nudged: bool) {
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": format!("AC4 {origin} {minutes_to_claim}m")}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("signup");
    let ns = result["tenant"].as_str().expect("tenant").to_string();
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");

    let now = mcphost::state::now_unix();
    let verified_at = now - 3600; // within the 30-day window
    let created_unix = verified_at - minutes_to_claim * 60;
    server
        .state
        .db
        .set_claim_timing_for_test(
            tenant.id,
            origin.to_string(),
            created_unix,
            Some(verified_at),
            if nudged { Some(created_unix + 60) } else { None },
        )
        .await
        .expect("set claim timing");
}

#[tokio::test]
async fn healthz_claims_block_counts_by_origin_and_computes_the_median() {
    let server = TestServer::start().await;

    claimed_tenant(&server, "external", 10, true).await;
    claimed_tenant(&server, "external", 20, false).await;
    claimed_tenant(&server, "external", 30, true).await;
    claimed_tenant(&server, "synthetic", 5, false).await;

    let body = healthz(&server.base_url).await;
    let claims = &body["claims"];
    assert_eq!(claims["external"], json!(3), "{claims}");
    assert_eq!(claims["synthetic"], json!(1), "{claims}");
    // Sorted minutes across all four: [5, 10, 20, 30] -> median (10+20)/2.
    assert_eq!(
        claims["median_minutes_to_claim"].as_f64(),
        Some(15.0),
        "{claims}"
    );
    assert_eq!(claims["nudged"], json!(2), "{claims}");
    assert_eq!(claims["nudged_then_claimed"], json!(2), "{claims}");
}
