//! PRD-mcphost-time-to-first-success-public AC7 (P1) — Given 6 external
//! tenants with `client_name` `cursor` and 2 with `windsurf`, When the block
//! is computed, Then `by_client.cursor` has percentiles and `windsurf` is
//! absent.

use crate::common;
use common::{TestServer, ttfs_tenant};
use serde_json::json;

#[tokio::test]
async fn groups_under_five_are_absent_and_larger_ones_have_percentiles() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = mcphost::state::now_unix();
    for i in 0..6 {
        ttfs_tenant(db, &format!("ttfs7-cur-{i}"), "external", Some("cursor"), now - 600, [Some(1), Some(10), Some(20 + i), None]).await;
    }
    for i in 0..2 {
        ttfs_tenant(db, &format!("ttfs7-win-{i}"), "external", Some("windsurf"), now - 600, [Some(1), Some(10), Some(20), None]).await;
    }
    let block = mcphost::funnel::onboarding_7d(db, now, mcphost::funnel::ONBOARDING_MIN_N).await.expect("compute");
    let by_client = block["by_client"].as_object().expect("by_client");
    assert_eq!(by_client["cursor"]["n"], json!(6), "{block}");
    assert!(by_client["cursor"]["p50_s_to_first_own_call"].is_number(), "{block}");
    assert!(!by_client.contains_key("windsurf"), "two windsurf tenants are below the floor: {block}");
    assert_eq!(block["n"], json!(8), "{block}");
}
