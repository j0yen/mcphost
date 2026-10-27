//! PRD-mcphost-tools-call-progress-notifications
//! AC1 (P0) — Given the two new `calls` columns and admin healthz, When a
//! `synthetic`-origin tenant makes one `tools/call` with
//! `_meta.progressToken` and one without, and an `external`-origin tenant
//! makes one with a token, all on a tool that never reports, Then
//! `progress_7d.calls_total` reads `{synthetic: 2, external: 1, total: 3}`,
//! `calls_with_token` reads `{synthetic: 1, external: 1, total: 2}`,
//! `frames_sent` reads `{synthetic: 0, external: 0, total: 0}`, and `since`
//! is the migration's unix time.

use crate::common;
use crate::tcpn;
use tcpn::{admin_healthz, ac1_fixture};
use serde_json::json;

#[tokio::test]
async fn tcpn_ac01_counters_split_by_origin() {
    let before = mcphost::state::now_unix();
    let server = ac1_fixture().await;
    let health = admin_healthz(&server.base_url).await;
    let p = &health["progress_7d"];
    // PRD-mcphost-external-class-positive-evidence AC1 supersedes the old
    // by-elimination rule: the public-IP signup with no evidence is `unattributed`.
    assert_eq!(p["calls_total"], json!({"synthetic": 2, "external": 0, "unattributed": 1, "total": 3}), "{p}");
    assert_eq!(p["calls_with_token"], json!({"synthetic": 1, "external": 0, "unattributed": 1, "total": 2}), "{p}");
    assert_eq!(p["frames_sent"], json!({"synthetic": 0, "external": 0, "unattributed": 0, "total": 0}), "{p}");
    let since = p["since"].as_i64().expect("since");
    let epoch = server.state.db.progress_epoch().await.expect("epoch");
    assert_eq!(since, epoch, "since is the migration's stored unix time: {p}");
    assert!((before - 60..=mcphost::state::now_unix()).contains(&since), "migration ran inside this test: {since} vs {before}");
}

/// `external` stays exercised: a public-IP tenant whose owner verified a claim
/// e-mail (positive evidence, `db::promote_origin_sync` ClaimedEmail) is
/// `external` before its call, so the call counts under `external`.
#[tokio::test]
async fn tcpn_ac01_counters_external_with_positive_evidence() {
    let server = common::TestServer::start_with_signup_rate_limit(20).await;
    let result = mcphost::control::signup(&server.state, &json!({"name": "tcpn evidenced"}), "203.0.113.12", mcphost::control::SignupAttribution::default())
        .await
        .expect("signup");
    let ns = result["tenant"].as_str().expect("tenant").to_string();
    let id = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("query").expect("tenant").id;
    server.state.db.verify_owner(id, "email".to_string(), "tcpn@example.com".to_string()).await.expect("verify_owner");
    let t = server.state.db.find_tenant_by_namespace(ns).await.expect("query").expect("tenant");
    assert_eq!(t.origin, "external", "claim e-mail verification is positive evidence");
    let client = common::McpClient::with_bearer(&server.base_url, result["key"].as_str().expect("key"));
    let tool = common::publish(&client, "hello", "echo", json!({"schema": {"type": "object"}})).await;
    let resp = tcpn::call_with_token(&client, &tool, json!({"msg": "hi"}), Some(json!("c"))).await;
    assert!(resp.status().is_success(), "{tool}: {}", resp.status());
    let p = &admin_healthz(&server.base_url).await["progress_7d"];
    assert_eq!(p["calls_total"], json!({"synthetic": 0, "external": 1, "total": 1}), "{p}");
    assert_eq!(p["calls_with_token"], json!({"synthetic": 0, "external": 1, "total": 1}), "{p}");
}
