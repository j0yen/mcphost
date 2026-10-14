//! PRD-mcphost-time-to-first-success-public AC5 (P0) — Given 4 external
//! tenants in the window, When the block is computed, Then `n == 4` and every
//! percentile and `under_60s_share` is null.

use crate::common;
use common::{TestServer, ttfs_tenant};
use serde_json::json;

#[tokio::test]
async fn four_external_tenants_publish_n_and_nothing_else() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = mcphost::state::now_unix();
    for i in 0..4 {
        ttfs_tenant(db, &format!("ttfs5-{i}"), "external", None, now - 600, [Some(1), Some(10), Some(20), None]).await;
    }
    let block = mcphost::funnel::onboarding_7d(db, now, mcphost::funnel::ONBOARDING_MIN_N).await.expect("compute");
    assert_eq!(block["n"], json!(4), "{block}");
    for key in [
        "p50_s_to_first_publish",
        "p95_s_to_first_publish",
        "p50_s_to_first_own_call",
        "p95_s_to_first_own_call",
        "under_60s_share",
    ] {
        assert!(block[key].is_null(), "{key} must be null below the floor: {block}");
    }

    // The same data shown through the public feed.
    let status: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse");
    assert_eq!(status["onboarding_7d"]["n"], json!(4), "{status}");
    assert!(status["onboarding_7d"]["under_60s_share"].is_null(), "{status}");
}
