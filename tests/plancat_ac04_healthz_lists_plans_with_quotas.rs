//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC4 (P0) — Given the loaded catalog, When admin healthz is read, Then
//! `plans[]` lists each plan with the six quota fields and non-zero
//! `state_bytes_max`.

use crate::common;
use common::{ADMIN_KEY, TestServer};

#[tokio::test]
async fn healthz_lists_every_plan_with_its_six_quota_fields() {
    let server = TestServer::start().await;

    let health: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");

    let plans = health["plans"].as_array().expect("plans[] must be an array");
    assert!(!plans.is_empty(), "plans[] must not be empty: {health:?}");

    for plan in plans {
        for field in [
            "name",
            "state_bytes_max",
            "state_rows_max",
            "table_bytes_max",
            "docs_bytes_max",
            "jobs_concurrent",
            "end_users_max",
        ] {
            assert!(
                plan.get(field).is_some(),
                "plan entry must carry {field}: {plan:?}"
            );
        }
        let state_bytes_max = plan["state_bytes_max"].as_i64().expect("state_bytes_max is a number");
        assert!(
            state_bytes_max > 0,
            "state_bytes_max must be non-zero: {plan:?}"
        );
    }
}
