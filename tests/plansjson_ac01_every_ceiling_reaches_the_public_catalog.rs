//! Hotfix (949 live AC 8): `/plans.json` and `billing.plans` must carry every
//! plan ceiling. AC1 (P0) -- Given the default catalog, When
//! `billing::plans_from_catalog` renders it, Then every `(name, value)` of
//! each plan's `ceilings_flat()` is a key of that plan's object with the same
//! value (generated from the same function, so the two cannot drift); and
//! `GET /plans.json` on the test server carries the exact AC 8 predicate,
//! `free.sandbox_memory_mb_max == 256`.

use crate::common::TestServer;
use mcphost::plans::PlanCatalog;
use serde_json::json;

#[test]
fn plansjson_ac01_every_ceiling_reaches_plans_from_catalog() {
    let catalog = PlanCatalog::default_catalog();
    let rendered = mcphost::billing::plans_from_catalog(&catalog, "off");
    let plans = rendered["plans"].as_array().expect("plans array");
    assert_eq!(plans.len(), catalog.plans.len());
    for plan in &catalog.plans {
        let obj = plans
            .iter()
            .find(|o| o["name"] == json!(plan.name))
            .unwrap_or_else(|| panic!("plan {} missing from plans_from_catalog", plan.name));
        let flat = plan.ceilings_flat();
        assert!(!flat.is_empty(), "{}: ceilings_flat is empty", plan.name);
        for (name, want) in flat {
            assert_eq!(
                obj[name.as_str()],
                json!(want),
                "plan {}: ceiling {name} missing from (or different in) the public catalog",
                plan.name
            );
        }
    }
}

#[tokio::test]
async fn plansjson_ac01_http_free_plan_carries_sandbox_memory_mb_max() {
    let server = TestServer::start().await;
    let body: serde_json::Value = reqwest::get(format!("{}/plans.json", server.base_url))
        .await
        .expect("GET /plans.json")
        .json()
        .await
        .expect("/plans.json body must be JSON");
    let free = body["plans"]
        .as_array()
        .expect("plans array")
        .iter()
        .find(|p| p["name"] == "free")
        .unwrap_or_else(|| panic!("free plan: {body}"));
    assert_eq!(free["sandbox_memory_mb_max"], json!(256), "{free}");
}
