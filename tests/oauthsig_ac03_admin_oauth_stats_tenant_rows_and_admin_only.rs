//! PRD-mcphost-oauth-demand-signal
//! AC3 (P0) — Given the same data as AC2, When `admin.oauth.demand_stats` runs,
//! Then the non-synthetic rows come first with their per-method counts,
//! synthetic rows carry `synthetic: true`, and a tenant key calling
//! `admin.oauth.demand_stats` is rejected like every other `admin.*` tool.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup_with_synthetic_header};
use serde_json::json;

#[tokio::test]
async fn tenants_are_non_synthetic_first_and_tenant_key_is_forbidden() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let now = mcphost::state::now_unix();
    let within_7d = now - 3600;

    // Same "real HTTP signup is always loopback -> synthetic" workaround
    // `oauthsig_ac02` already uses -- two non-synthetic tenants signed up
    // via `control::signup` directly with a fabricated external IP.
    async fn signup_external(state: &mcphost::state::AppState, name: &str) -> mcphost::db::Tenant {
        let result = mcphost::control::signup(
            state,
            &json!({"name": name}),
            "203.0.113.12",
            mcphost::control::SignupAttribution::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("external signup '{name}' failed: {e:?}"));
        let namespace = result["tenant"].as_str().expect("tenant field").to_string();
        state.db.find_tenant_by_namespace(namespace).await.unwrap().unwrap()
    }
    let tenant_a = signup_external(&server.state, "Non-Synthetic Tenant A").await;
    let tenant_b = signup_external(&server.state, "Non-Synthetic Tenant B").await;
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(tenant_a.id, "issuer_jwt", within_7d)
        .await
        .expect("seed A's issuer_jwt call");
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(tenant_b.id, "key", within_7d)
        .await
        .expect("seed B's key call");

    // One synthetic tenant, calling with a hosted token.
    let signup_result =
        signup_with_synthetic_header(&server.base_url, "Synthetic Tenant", "harness:unstamped").await;
    let synthetic_namespace = signup_result["tenant"].as_str().expect("tenant field").to_string();
    let synthetic_tenant = server.state.db.find_tenant_by_namespace(synthetic_namespace).await.unwrap().unwrap();
    server
        .state
        .db
        .insert_calls_row_with_auth_method_for_test(synthetic_tenant.id, "hosted_token", within_7d)
        .await
        .expect("seed synthetic tenant's hosted_token call");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin.tools_call("admin.oauth.demand_stats", json!({})).await.expect("admin.oauth.demand_stats");
    let body = extract_structured(&result);
    let tenants = body["tenants"].as_array().expect("tenants array");

    let synthetic_flags: Vec<bool> = tenants
        .iter()
        .map(|t| t["synthetic"].as_bool().expect("synthetic is a bool"))
        .collect();
    assert!(
        synthetic_flags.is_sorted(),
        "every non-synthetic (false) row must sort before every synthetic (true) row: {synthetic_flags:?}"
    );

    let row_a = tenants
        .iter()
        .find(|t| t["tenant_id"].as_i64() == Some(tenant_a.id))
        .unwrap_or_else(|| panic!("tenant A's row missing: {tenants:?}"));
    assert_eq!(row_a["synthetic"], json!(false), "{row_a:?}");
    assert_eq!(row_a["calls_7d"]["issuer_jwt"], json!(1), "{row_a:?}");
    assert_eq!(row_a["calls_7d"]["key"], json!(0), "{row_a:?}");

    let row_b = tenants
        .iter()
        .find(|t| t["tenant_id"].as_i64() == Some(tenant_b.id))
        .unwrap_or_else(|| panic!("tenant B's row missing: {tenants:?}"));
    assert_eq!(row_b["synthetic"], json!(false), "{row_b:?}");
    assert_eq!(row_b["calls_7d"]["key"], json!(1), "{row_b:?}");

    let synthetic_row = tenants
        .iter()
        .find(|t| t["tenant_id"].as_i64() == Some(synthetic_tenant.id))
        .unwrap_or_else(|| panic!("synthetic tenant's row missing: {tenants:?}"));
    assert_eq!(synthetic_row["synthetic"], json!(true), "{synthetic_row:?}");
    assert_eq!(synthetic_row["calls_7d"]["hosted_token"], json!(1), "{synthetic_row:?}");

    // A tenant key calling `admin.oauth.demand_stats` is rejected exactly like
    // every other `admin.*` tool (vaultst_ac05's own precedent).
    let (_ns_c, tenant_key) = common::signup(&server.base_url, "Ordinary Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &tenant_key);
    let err = tenant_client
        .tools_call("admin.oauth.demand_stats", json!({}))
        .await
        .expect_err("a tenant key must never reach admin.oauth.demand_stats");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
}
