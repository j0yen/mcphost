//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC1 (P0) — Given a `plans.toml` with `free` and `pro` naming only
//! `price_usd_month`, `tools_max`, `calls_per_day`, `secrets_max`,
//! `description`, When the catalog loads, Then `free.state_bytes_max ==
//! 5 MiB`, `free.state_rows_max == 10000`, `pro.state_bytes_max ==
//! 200 MiB`, and every other quota equals `default_catalog()`'s value for
//! that plan.

use mcphost::plans::PlanCatalog;

const PLANS_TOML: &str = "\
[[plan]]
name = \"free\"
price_usd_month = 0
tools_max = 50
calls_per_day = 500
secrets_max = 2
description = \"Free: 50 tools, 500 calls/day, 2 secrets. No card required.\"

[[plan]]
name = \"pro\"
price_usd_month = 19
tools_max = 50
calls_per_day = 100000
secrets_max = 20
description = \"Pro: 50 tools, 100,000 calls/day, 20 secrets. $19/month, 50,000 calls included per month, then usage-billed.\"
";

#[test]
fn pre_state_quota_file_defaults_every_unnamed_quota_from_default_catalog() {
    let (catalog, _defaulted) = PlanCatalog::from_toml(PLANS_TOML).expect("parse");
    let defaults = PlanCatalog::default_catalog();

    let free = catalog.get("free").expect("free plan");
    let free_defaults = defaults.get("free").expect("free defaults");
    assert_eq!(free.state_bytes_max, 5 * 1024 * 1024);
    assert_eq!(free.state_rows_max, 10_000);
    assert_eq!(free, free_defaults, "every unnamed quota must equal default_catalog()'s free row");

    let pro = catalog.get("pro").expect("pro plan");
    let pro_defaults = defaults.get("pro").expect("pro defaults");
    assert_eq!(pro.state_bytes_max, 200 * 1024 * 1024);
    assert_eq!(pro, pro_defaults, "every unnamed quota must equal default_catalog()'s pro row");
}
