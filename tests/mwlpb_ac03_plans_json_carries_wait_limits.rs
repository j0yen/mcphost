//! PRD-mcphost-msg-wait-long-poll-contract-b
//! AC3 (P0) — Given `/plans.json` on a built binary, When fetched, Then
//! every plan carries `wait_max_s: 25` and `wait_default_s: 20`.

use crate::common;
use common::TestServer;
use serde_json::Value;

#[tokio::test]
async fn every_plan_carries_wait_max_and_default() {
    let server = TestServer::start().await;
    let body: Value = reqwest::get(format!("{}/plans.json", server.base_url))
        .await
        .expect("GET /plans.json")
        .json()
        .await
        .expect("json body");
    let plans = body["plans"].as_array().unwrap_or_else(|| panic!("plans array: {body}"));
    assert!(!plans.is_empty(), "{body}");
    for plan in plans {
        assert_eq!(plan["wait_max_s"], 25, "{plan}");
        assert_eq!(plan["wait_default_s"], 20, "{plan}");
    }
}
