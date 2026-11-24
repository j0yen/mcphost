//! PRD-mcphost-response-size-budget-explicit-v2
//! AC1 (P0) -- Given a page cut for budget on a paged verb, When
//! `rszb_ac01_one_truncation_struct` serializes `_meta.mcphost.truncated`,
//! Then it has exactly the keys `bytes`, `of` and `continue_with`, and a
//! response that was also clamped by `limit` holds both `limit_clamped` and
//! `truncated` under `_meta.mcphost`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

const BUDGET: i64 = 4096;

async fn seed_runs(server: &TestServer, tenant_id: i64, n: usize) {
    for i in 0..n {
        server
            .state
            .db
            .insert_queued_run(
                format!("rszb-ac1-run-{i:03}"),
                tenant_id,
                "ghost".to_string(),
                "event".to_string(),
                None,
                None,
                300,
                "{}".to_string(),
                false,
                false,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("insert_queued_run");
    }
}

#[tokio::test]
async fn rszb_ac01_one_truncation_struct() {
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            plan.response_budget_bytes = BUDGET;
        }
    })
    .await;
    let (ns, key) = signup(&server.base_url, "Rszb AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db lookup").expect("tenant exists");
    seed_runs(&server, tenant.id, 60).await;

    // A page that fits carries no marker at all.
    let small = client.tools_call("host.runs.list", json!({"limit": 2})).await.expect("small page");
    assert!(small["_meta"].get("mcphost").is_none_or(serde_json::Value::is_null), "{small:?}");

    // A page over the budget is cut and says so, with the one struct.
    let cut = client.tools_call("host.runs.list", json!({"limit": 200})).await.expect("cut page");
    let body = common::extract_structured(&cut);
    let sent = body["runs"].as_array().expect("runs").len();
    assert!((1..60).contains(&sent), "the page was cut at a row boundary: sent {sent}: {cut:?}");
    let marker = cut["_meta"]["mcphost"]["truncated"].as_object().unwrap_or_else(|| panic!("truncated: {cut:?}"));
    let mut keys: Vec<&str> = marker.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["bytes", "continue_with", "of"], "{marker:?}");
    let bytes = marker["bytes"].as_u64().expect("bytes");
    let of = marker["of"].as_u64().expect("of");
    assert!(bytes <= BUDGET as u64 && bytes < of, "bytes {bytes} of {of}");
    let next = &marker["continue_with"];
    assert_eq!(next["verb"], json!("host.runs.list"), "{next}");
    assert_eq!(next["args"]["cursor"], body["next_cursor"], "{next}");
    assert!(body.get(mcphost::response_budget::TRUNCATED_KEY).is_none(), "the carrier key must not leak: {body}");

    // Also clamped by `limit`: both keys sit side by side in `_meta.mcphost`.
    let both = client.tools_call("host.runs.list", json!({"limit": 5000})).await.expect("clamped and cut");
    let mcphost = both["_meta"]["mcphost"].as_object().unwrap_or_else(|| panic!("mcphost: {both:?}"));
    assert_eq!(mcphost["limit_clamped"], json!({"asked": 5000, "max": 200}), "{mcphost:?}");
    assert!(mcphost["truncated"].is_object(), "{mcphost:?}");
    assert_eq!(mcphost.len(), 2, "{mcphost:?}");

    // The struct itself serializes to exactly those three keys.
    let direct = mcphost::response_budget::Truncated { bytes: 1, of: 2, continue_with: None }.to_json();
    let mut keys: Vec<&str> = direct.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["bytes", "continue_with", "of"], "{direct}");
}
