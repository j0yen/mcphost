//! PRD-mcphost-pricing-page-from-plan-table AC7 (P0) -- Given a `plans.toml`
//! without the two fields, When the host starts, Then the log line
//! `plans.toml: plan=pro defaulted=[` names both and `/plans.json` shows the
//! defaults.

use crate::busyaudit;
use crate::common::TestServer;

use busyaudit::{capture_tracing, scratch_data_dir};
use mcphost::plans::PlanCatalog;

const PLANS_TOML: &str = "\
[[plan]]
name = \"free\"
price_usd_month = 0
tools_max = 50
calls_per_day = 500
secrets_max = 2
description = \"Free tier\"

[[plan]]
name = \"pro\"
price_usd_month = 19
tools_max = 50
calls_per_day = 100000
secrets_max = 20
description = \"Pro tier\"
";

#[tokio::test]
async fn old_plans_toml_logs_both_fields_defaulted_and_plans_json_shows_defaults() {
    let dir = scratch_data_dir("ppage-ac07");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(&path, PLANS_TOML).unwrap();

    let (result, log) = capture_tracing(|| PlanCatalog::load_or_init(&path));
    let loaded = result.expect("load_or_init");
    std::fs::remove_dir_all(&dir).ok();

    let pro_line = log
        .lines()
        .find(|l| l.contains("plans.toml: plan=pro defaulted=["))
        .unwrap_or_else(|| panic!("no pro defaulted= line in:\n{log}"));
    for field in ["calls_included_month", "overage_usd_per_1000_calls"] {
        assert!(pro_line.contains(field), "pro's defaulted= line must name {field}: {pro_line}");
    }

    let server = TestServer::start_with_plans(move |catalog| *catalog = loaded).await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/plans.json", server.base_url))
        .send()
        .await
        .expect("GET /plans.json")
        .json()
        .await
        .expect("json");
    let plans = body["plans"].as_array().expect("plans");
    let pro = plans.iter().find(|p| p["name"] == "pro").expect("pro");
    assert_eq!(pro["calls_included_month"], 50_000);
    assert_eq!(pro["overage_usd_per_1000_calls"], 1);
    let free = plans.iter().find(|p| p["name"] == "free").expect("free");
    assert_eq!(free["calls_included_month"], 0);
    assert_eq!(free["overage_usd_per_1000_calls"], 0);
}
