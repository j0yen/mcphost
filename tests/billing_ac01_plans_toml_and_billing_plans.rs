//! AC1 — Given a fresh data dir, When mcphost starts, Then `plans.toml`
//! exists with `free` and `pro` rows and `billing.plans` returns both with
//! prices and quotas and `billing_mode: off`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::plans::PlanCatalog;
use serde_json::json;

/// The file-level half of AC1: a fresh data dir has no `plans.toml` until
/// `PlanCatalog::load_or_init` (main.rs's own startup call) writes one with
/// the documented `free`/`pro` defaults.
#[test]
fn fresh_data_dir_gets_plans_toml_with_free_and_pro() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-billing-ac1-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = PlanCatalog::path_from_env(&dir);
    assert!(!path.exists(), "must start absent for this test to prove anything");

    let catalog = PlanCatalog::load_or_init(&path).expect("load_or_init");
    assert!(path.exists(), "plans.toml must exist after startup");

    // PRD-mcphost-metered-overage AC13: the published-numbers alignment.
    let free = catalog.get("free").expect("free plan row");
    assert_eq!(free.price_usd_month, 0);
    assert_eq!(free.tools_max, 50);
    assert_eq!(free.calls_per_day, 500);
    assert_eq!(free.secrets_max, 2);

    let pro = catalog.get("pro").expect("pro plan row");
    assert_eq!(pro.price_usd_month, 19);
    assert_eq!(pro.tools_max, 50);
    assert_eq!(pro.calls_per_day, 100_000);
    assert_eq!(pro.secrets_max, 20);

    std::fs::remove_dir_all(&dir).ok();
}

/// The MCP-surface half of AC1: `billing.plans` (anonymous and tenant) sees
/// both rows and reports `billing_mode: off` when no Stripe key is
/// configured -- the default every `TestServer::start*` helper builds
/// except `start_with_billing`.
#[tokio::test]
async fn billing_plans_lists_both_rows_with_billing_mode_off() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);

    let result = anon
        .tools_call("billing.plans", json!({}))
        .await
        .expect("billing.plans should succeed unauthenticated");
    let structured = extract_structured(&result);

    assert_eq!(structured["billing_mode"], json!("off"));
    let plans = structured["plans"].as_array().expect("plans array");
    let names: Vec<&str> = plans.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"free"), "{names:?}");
    assert!(names.contains(&"pro"), "{names:?}");

    let free = plans.iter().find(|p| p["name"] == "free").unwrap();
    assert_eq!(free["price_usd_month"], json!(0));
    assert_eq!(free["tools_max"], json!(50));
    assert_eq!(free["calls_per_day"], json!(500));
    assert_eq!(free["secrets_max"], json!(2));

    let pro = plans.iter().find(|p| p["name"] == "pro").unwrap();
    assert_eq!(pro["price_usd_month"], json!(19));
    assert_eq!(pro["tools_max"], json!(50));
    assert_eq!(pro["calls_per_day"], json!(100_000));
    assert_eq!(pro["secrets_max"], json!(20));
}

/// A signed-up tenant gets the exact same catalog -- "anonymous and
/// tenant" (AC1 wording).
#[tokio::test]
async fn billing_plans_is_identical_for_a_signed_up_tenant() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Plan Reader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("billing.plans", json!({}))
        .await
        .expect("billing.plans should succeed for a tenant");
    let structured = extract_structured(&result);
    assert_eq!(structured["billing_mode"], json!("off"));
    assert_eq!(structured["plans"].as_array().unwrap().len(), 2);
}
