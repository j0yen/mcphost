//! PRD-mcphost-tools-call-progress-notifications
//! AC1 (P0) — Given the two new `calls` columns and admin healthz, When a
//! `synthetic`-origin tenant makes one `tools/call` with
//! `_meta.progressToken` and one without, and an `external`-origin tenant
//! makes one with a token, all on a tool that never reports, Then
//! `progress_7d.calls_total` reads `{synthetic: 2, external: 1, total: 3}`,
//! `calls_with_token` reads `{synthetic: 1, external: 1, total: 2}`,
//! `frames_sent` reads `{synthetic: 0, external: 0, total: 0}`, and `since`
//! is the migration's unix time.

use crate::tcpn;
use tcpn::{admin_healthz, ac1_fixture};
use serde_json::json;

#[tokio::test]
async fn tcpn_ac01_counters_split_by_origin() {
    let before = mcphost::state::now_unix();
    let server = ac1_fixture().await;
    let health = admin_healthz(&server.base_url).await;
    let p = &health["progress_7d"];
    assert_eq!(p["calls_total"], json!({"synthetic": 2, "external": 1, "total": 3}), "{p}");
    assert_eq!(p["calls_with_token"], json!({"synthetic": 1, "external": 1, "total": 2}), "{p}");
    assert_eq!(p["frames_sent"], json!({"synthetic": 0, "external": 0, "total": 0}), "{p}");
    let since = p["since"].as_i64().expect("since");
    let epoch = server.state.db.progress_epoch().await.expect("epoch");
    assert_eq!(since, epoch, "since is the migration's stored unix time: {p}");
    assert!((before - 60..=mcphost::state::now_unix()).contains(&since), "migration ran inside this test: {since} vs {before}");
}
