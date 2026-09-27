//! PRD-mcphost-oauth-demand-signal
//! AC2 (P0) — Given three non-synthetic tenants (two calling with issuer
//! JWTs, one with a key) and five synthetic tenants calling with hosted
//! tokens within 7 days, When admin healthz is read, Then `oauth.calls_7d`
//! counts every call by method, `oauth.tenants_7d == {issuer_jwt: 2,
//! hosted_token: 0}`, and `first_issuer_jwt_call_at` equals the earliest
//! such call.
//!
//! Seeds `calls` rows directly via `Db::insert_calls_row_with_auth_method_for_test`
//! rather than driving a real bearer through the whole dispatch path per
//! row -- `oauthsig_ac01` already proves the real credential -> auth_method
//! resolution end to end; this AC is about the READ-side aggregation.

use crate::common;
use common::{ADMIN_KEY, TestServer, signup_with_synthetic_header};
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

#[tokio::test]
async fn healthz_oauth_counts_calls_and_non_synthetic_tenants_by_method() {
    // Eight signups (3 non-synthetic + 5 synthetic) from this test's own
    // loopback source would otherwise trip the default 5/hour signup rate
    // limit (`state::SIGNUP_RATE_LIMIT_PER_HOUR`).
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let now = mcphost::state::now_unix();
    let within_7d = now - 3600;

    // Three non-synthetic tenants: two called with an issuer JWT, one with
    // a key. A real HTTP signup against this test server is always a
    // loopback connection (`state::classify_source_class` -> synthetic --
    // same workaround `provaudit_ac04_healthz_split_shape.rs` already uses),
    // so these three are signed up via `control::signup` directly with a
    // fabricated external source IP instead of `common::signup`'s real HTTP
    // round trip.
    async fn signup_external(state: &mcphost::state::AppState, name: &str) -> mcphost::db::Tenant {
        let result = mcphost::control::signup(
            state,
            &json!({"name": name}),
            "203.0.113.11",
            mcphost::control::SignupAttribution::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("external signup '{name}' failed: {e:?}"));
        let namespace = result["tenant"].as_str().expect("tenant field").to_string();
        state.db.find_tenant_by_namespace(namespace).await.unwrap().unwrap()
    }
    let tenant_a = signup_external(&server.state, "Issuer JWT Tenant A").await;
    let tenant_b = signup_external(&server.state, "Issuer JWT Tenant B").await;
    let tenant_c = signup_external(&server.state, "Key Tenant C").await;

    // Tenant A's earliest issuer_jwt call is the one `first_issuer_jwt_call_at`
    // must report -- seeded strictly earlier than B's or A's own second call.
    let earliest_issuer_jwt_call = now - 6 * 86_400;
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(tenant_a.id, "issuer_jwt", earliest_issuer_jwt_call)
        .await
        .expect("seed A's earliest issuer_jwt call");
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(tenant_a.id, "issuer_jwt", within_7d)
        .await
        .expect("seed A's second issuer_jwt call");
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(tenant_b.id, "issuer_jwt", within_7d)
        .await
        .expect("seed B's issuer_jwt call");
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(tenant_c.id, "key", within_7d)
        .await
        .expect("seed C's key call");

    // Five synthetic tenants, each calling with a hosted token within 7
    // days -- must inflate `calls_7d.hosted_token` but NOT `tenants_7d.hosted_token`.
    for i in 0..5 {
        let signup_result =
            signup_with_synthetic_header(&server.base_url, &format!("Synthetic Tenant {i}"), "harness:unstamped")
                .await;
        let namespace = signup_result["tenant"].as_str().expect("tenant field").to_string();
        let tenant = server.state.db.find_tenant_by_namespace(namespace).await.unwrap().unwrap();
        server
            .state
            .db
            .insert_calls_row_with_auth_method_for_test(tenant.id, "hosted_token", within_7d)
            .await
            .expect("seed synthetic tenant's hosted_token call");
    }

    let health = healthz(&server.base_url).await;
    let oauth = health.get("oauth").unwrap_or_else(|| panic!("missing 'oauth' in {health:?}"));

    assert_eq!(oauth["calls_7d"]["key"], json!(1), "{oauth:?}");
    assert_eq!(oauth["calls_7d"]["issuer_jwt"], json!(3), "{oauth:?}");
    assert_eq!(oauth["calls_7d"]["hosted_token"], json!(5), "{oauth:?}");

    assert_eq!(
        oauth["tenants_7d"],
        json!({"issuer_jwt": 2, "hosted_token": 0}),
        "synthetic hosted_token callers must be excluded from tenants_7d: {oauth:?}"
    );

    assert_eq!(
        oauth["first_issuer_jwt_call_at"],
        json!(earliest_issuer_jwt_call),
        "{oauth:?}"
    );
}
