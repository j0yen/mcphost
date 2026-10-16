//! PRD-mcphost-pricing-page-from-plan-table AC2 (P0) -- Given `/plans.json`
//! from the test server, When read, Then every plan carries both fields and
//! no plan's `description` contains a digit.

use crate::common::TestServer;

#[tokio::test]
async fn plans_json_plans_carry_both_fields_and_digit_free_descriptions() {
    let server = TestServer::start().await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/plans.json", server.base_url))
        .send()
        .await
        .expect("GET /plans.json")
        .json()
        .await
        .expect("/plans.json body must be JSON");
    let plans = body["plans"].as_array().expect("plans array");
    assert!(!plans.is_empty());
    for plan in plans {
        for field in ["calls_included_month", "overage_usd_per_1000_calls"] {
            assert!(plan[field].is_i64(), "plan {} lacks integer {field}: {plan}", plan["name"]);
        }
        let description = plan["description"].as_str().expect("description string");
        assert!(
            !description.chars().any(|c| c.is_ascii_digit()),
            "plan {} description carries a digit: {description}",
            plan["name"]
        );
    }
    let pro = plans.iter().find(|p| p["name"] == "pro").expect("pro");
    assert_eq!(pro["calls_included_month"], 50_000);
    assert_eq!(pro["overage_usd_per_1000_calls"], 1);
}
