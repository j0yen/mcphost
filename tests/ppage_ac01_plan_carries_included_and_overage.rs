//! PRD-mcphost-pricing-page-from-plan-table AC1 (P0) -- Given the PRD's
//! `Plan`, When `default_catalog()` is read, Then pro has `50000` and `1`,
//! free has `0` and `0`, and `ceilings_json()` contains neither name.

use mcphost::plans::{NON_CEILING_FIELDS, PlanCatalog};

#[test]
fn default_catalog_carries_included_volume_and_overage_rate() {
    let catalog = PlanCatalog::default_catalog();
    let pro = catalog.get("pro").expect("pro plan");
    assert_eq!(pro.calls_included_month, 50_000);
    assert_eq!(pro.overage_usd_per_1000_calls, 1);
    let free = catalog.get("free").expect("free plan");
    assert_eq!(free.calls_included_month, 0);
    assert_eq!(free.overage_usd_per_1000_calls, 0);
}

#[test]
fn ceilings_json_contains_neither_price_field() {
    let catalog = PlanCatalog::default_catalog();
    for plan in &catalog.plans {
        let text = plan.ceilings_json().to_string();
        for name in ["calls_included_month", "overage_usd_per_1000_calls"] {
            assert!(!text.contains(name), "{}: ceilings_json leaks {name}: {text}", plan.name);
            assert!(NON_CEILING_FIELDS.contains(&name), "{name} must be a NON_CEILING_FIELD");
        }
    }
}
