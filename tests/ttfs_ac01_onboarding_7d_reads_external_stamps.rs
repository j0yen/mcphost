//! PRD-mcphost-time-to-first-success-public AC1 (P0) — Given three fixture
//! tenants with `origin = external` and stamps at 10 s, 50 s and 400 s to
//! `first_own_call`, plus two fleet tenants at 1 s, When `onboarding_7d` is
//! computed, Then `n == 3`, `p50_s_to_first_own_call == 50`,
//! `under_60s_share == 0.667`, and the fleet rows are not counted.

use crate::common;
use common::{TestServer, ttfs_tenant};
use serde_json::json;

#[tokio::test]
async fn external_stamps_are_what_is_read_and_fleet_rows_are_not_counted() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = mcphost::state::now_unix();
    let born = now - 3600;
    for (i, secs) in [10, 50, 400].into_iter().enumerate() {
        ttfs_tenant(db, &format!("ttfs1-ext-{i}"), "external", None, born, [Some(1), Some(5), Some(secs), None]).await;
    }
    for i in 0..2 {
        ttfs_tenant(db, &format!("ttfs1-fleet-{i}"), "synthetic", None, born, [Some(1), Some(1), Some(1), None]).await;
    }

    // Computation half: the minimum-n floor lowered to 1 so three tenants show.
    let block = mcphost::funnel::onboarding_7d(db, now, 1).await.expect("compute");
    assert_eq!(block["n"], json!(3), "{block}");
    assert_eq!(block["p50_s_to_first_own_call"], json!(50), "{block}");
    assert_eq!(block["under_60s_share"].as_f64(), Some(0.667), "{block}");
    assert_eq!(block["stamped_n"]["first_own_call"], json!(3), "{block}");

    // The public path keeps the floor: three tenants publish `n` and nulls only.
    let public = mcphost::funnel::onboarding_7d(db, now, mcphost::funnel::ONBOARDING_MIN_N).await.expect("compute");
    assert_eq!(public["n"], json!(3), "{public}");
    assert!(public["p50_s_to_first_own_call"].is_null(), "{public}");
}
