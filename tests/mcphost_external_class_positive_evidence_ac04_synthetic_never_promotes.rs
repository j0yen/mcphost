//! PRD-mcphost-external-class-positive-evidence
//! AC4 — Given a `synthetic` tenant, When a claim is verified or a qualifying
//! later-day call arrives, Then `origin` stays `synthetic`, no
//! `origin_promoted` audit row is written, and `synthetic_promotion_refused`
//! increments and is readable in healthz.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use mcphost::db::{OriginCallObservation, OriginEvidence, PromoteOutcome};
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac04() {
    let server = TestServer::start().await;
    let before = healthz(&server.base_url).await;
    assert_eq!(before["synthetic_promotion_refused"], json!(0), "{before:?}");

    // A loopback signup is synthetic; give it a verified owner and an old age.
    let (ns, _key) = common::signup(&server.base_url, "AC4 Synthetic").await;
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "synthetic");
    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .set_claim_timing_for_test(t.id, "synthetic".to_string(), now - 3 * 86_400, Some(now), None)
        .await
        .unwrap();

    // A verified claim is refused (the conjunctive predicate keeps it synthetic).
    let out = server.state.db.promote_origin(t.id, OriginEvidence::ClaimedEmail).await.unwrap();
    assert_eq!(out, PromoteOutcome::RefusedSynthetic);
    // A qualifying later-day clean call is refused too.
    let out = server
        .state
        .db
        .observe_origin_call(t.id, now, OriginCallObservation::Clean { ip_hash: "h".to_string() })
        .await
        .unwrap();
    assert_eq!(out, PromoteOutcome::RefusedSynthetic);

    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "synthetic");
    let promoted = server
        .state
        .db
        .list_admin_audit(1000, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted")
        .count();
    assert_eq!(promoted, 0, "no origin_promoted row for a synthetic tenant");

    let after = healthz(&server.base_url).await;
    assert_eq!(after["synthetic_promotion_refused"], json!(2), "{after:?}");

    // Through the call path: a clean forwarded call on the synthetic tenant
    // increments the counter and leaves the origin alone.
    let (ns2, key2) = common::signup(&server.base_url, "AC4 Path").await;
    let t2 = server.state.db.find_tenant_by_namespace(ns2.clone()).await.unwrap().unwrap();
    server
        .state
        .db
        .set_claim_timing_for_test(t2.id, "synthetic".to_string(), now - 3 * 86_400, None, None)
        .await
        .unwrap();
    McpClient::with_bearer(&server.base_url, &key2)
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "203.0.113.41")])
        .await
        .expect("whoami");
    let after = healthz(&server.base_url).await;
    assert_eq!(after["synthetic_promotion_refused"], json!(3), "{after:?}");
    let t2 = server.state.db.find_tenant_by_namespace(ns2).await.unwrap().unwrap();
    assert_eq!(t2.origin, "synthetic");
}
