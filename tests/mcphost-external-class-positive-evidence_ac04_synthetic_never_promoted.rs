//! PRD-mcphost-external-class-positive-evidence
//! AC4 — Given a `synthetic` tenant, When a claim is verified or a
//! qualifying later-day call arrives, Then `origin` stays `synthetic`, no
//! `origin_promoted` audit row is written, and `synthetic_promotion_refused`
//! increments and is readable in healthz.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::{Value, json};

async fn refused(server: &TestServer) -> i64 {
    let health: Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    health["synthetic_promotion_refused"].as_i64().expect("synthetic_promotion_refused in healthz")
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac04_synthetic_stays_synthetic_and_refusals_count() {
    let server = TestServer::start().await;
    // A loopback signup is `synthetic`.
    let (namespace, key) = common::signup(&server.base_url, "AC4 Synthetic").await;
    let tenant = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(tenant.origin, "synthetic");
    assert_eq!(refused(&server).await, 0);

    // A verified claim does not promote it.
    server
        .state
        .db
        .verify_owner(tenant.id, "dns".to_string(), "ac4.example".to_string())
        .await
        .expect("verify");
    assert_eq!(refused(&server).await, 1);

    // Neither does a qualifying later-day, non-fleet, header-less call.
    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .set_claim_timing_for_test(tenant.id, "synthetic".to_string(), now - 3 * 86_400, None, None)
        .await
        .unwrap();
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "198.51.100.40")])
        .await
        .expect("authenticated call");
    assert_eq!(refused(&server).await, 2);

    let after = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(after.origin, "synthetic");
    let promoted = server
        .state
        .db
        .list_admin_audit(1000, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted")
        .count();
    assert_eq!(promoted, 0);
}
