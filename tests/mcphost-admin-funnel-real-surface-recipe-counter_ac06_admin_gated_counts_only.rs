//! PRD-mcphost-admin-funnel-real-surface-recipe-counter
//! AC6 (P0) — Given a non-admin tenant key and an anonymous caller, When each
//! calls `admin.funnel` and `/healthz`, Then the tenant key is refused as
//! today, an admin response holds no tenant id, namespace or client name
//! inside `by_surface` or `recipe_attempts_7d`, and the anonymous keys equal
//! `ANON_HEALTHZ_KEYS`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use mcphost::api_contract::SURFACE_STAGE_KEYS;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::kinds::aliases::recipe_names;
use serde_json::{Value, json};

#[tokio::test]
async fn tenant_and_anonymous_are_refused_and_the_admin_read_is_counts_only() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Recipe Counter AC6 Tenant").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    // A loopback signup is itself synthetic, so the counted tenant is an
    // ordinary human one created directly.
    let ordinary_ns = generate_namespace();
    let ordinary = server
        .state
        .db
        .create_tenant_attributed_with_source(
            "Ordinary Distinct Name".to_string(),
            ordinary_ns.clone(),
            hash_key(&generate_key()),
            None,
            Some("external".to_string()),
            Some("cursor".to_string()),
            None,
            "external".to_string(),
            None,
            None,
            "human".to_string(),
        )
        .await
        .expect("create tenant");
    server
        .state
        .db
        .insert_recipe_use(ordinary.id, recipe_names()[0].to_string(), "quickstart", mcphost::state::now_unix())
        .await
        .expect("use");

    // The tenant key is refused as today.
    let err = tenant.tools_call("admin.funnel", json!({"days": 7})).await.expect_err("tenant key refused");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
    // An anonymous caller is refused too.
    let anon = McpClient::new(&server.base_url);
    anon.tools_call("admin.funnel", json!({"days": 7})).await.expect_err("anonymous refused");

    // The admin read: integers under const keys and recipe names only.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let body = extract_structured(&admin.tools_call("admin.funnel", json!({"days": 7})).await.expect("admin.funnel"));
    let rendered = format!("{} {}", body["by_surface"], body["recipe_attempts_7d"]);
    for secret in [ns.as_str(), ordinary_ns.as_str(), "Ordinary Distinct Name", "Recipe Counter AC6 Tenant", "tenant_id", "namespace", "client_name"] {
        assert!(!rendered.contains(secret), "{secret} leaked into {rendered}");
    }
    for row in body["by_surface"].as_object().expect("by_surface").values() {
        for (k, v) in row.as_object().expect("row") {
            assert!(SURFACE_STAGE_KEYS.contains(&k.as_str()), "unexpected key {k}");
            assert!(v.is_i64() || v.is_null(), "{k}: {v}");
        }
    }
    let attempts = body["recipe_attempts_7d"].as_object().expect("recipe_attempts_7d");
    assert_eq!(attempts.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(), recipe_names().into_iter().collect());
    assert!(attempts.values().all(Value::is_i64));
    assert_eq!(attempts[recipe_names()[0]], json!(1));

    // The anonymous /healthz keeps exactly ANON_HEALTHZ_KEYS.
    let healthz: Value = reqwest::get(format!("{}/healthz", server.base_url)).await.expect("GET /healthz").json().await.expect("json");
    common::assert_anon_healthz(&healthz, true);
}
