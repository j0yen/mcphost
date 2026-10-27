//! PRD-mcphost-external-class-positive-evidence
//! AC4 (P0) — Given a `synthetic` tenant, When a claim is verified or a
//! qualifying later-day call arrives, Then `origin` stays `synthetic`, no
//! `origin_promoted` audit row is written, and `synthetic_promotion_refused`
//! increments and is readable in healthz.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use mcphost::state::now_unix;
use serde_json::{Value, json};
use std::sync::Arc;

async fn healthz(base_url: &str) -> Value {
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

async fn no_promotion_rows(server: &TestServer) {
    let rows: Vec<_> = server
        .state
        .db
        .list_admin_audit(500, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted")
        .collect();
    assert!(rows.is_empty(), "{rows:?}");
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac04_verified_claim_on_synthetic_is_refused() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    assert_eq!(healthz(&server.base_url).await["synthetic_promotion_refused"], json!(0));

    // A loopback signup is synthetic.
    let result = McpClient::new(&server.base_url)
        .tools_call("signup", json!({"name": "AC4 Synthetic"}))
        .await
        .expect("signup");
    let result = common::extract_structured(&result);
    let ns = result["tenant"].as_str().unwrap().to_string();
    let token = result["claim_url"].as_str().expect("claim_url").rsplit('/').next().unwrap().to_string();
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "synthetic");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "ac4@example.com")])
        .send()
        .await
        .unwrap();
    let body = &fake.sends()[0].text_body;
    let idx = body.find("/claim/verify/").expect("verify URL");
    let code = body[idx + "/claim/verify/".len()..].split_whitespace().next().unwrap().to_string();
    let resp = http.get(format!("{}/claim/verify/{code}", server.base_url)).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let t = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();
    assert!(t.owner_verified_at.is_some(), "the claim itself still verifies");
    assert_eq!(t.origin, "synthetic");
    no_promotion_rows(&server).await;
    assert_eq!(healthz(&server.base_url).await["synthetic_promotion_refused"], json!(1));
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac04_qualifying_later_day_call_on_synthetic_is_refused() {
    let server = TestServer::start().await;
    let (ns, key) = common::signup(&server.base_url, "AC4 Returner").await;
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "synthetic");
    server
        .state
        .db
        .set_claim_timing_for_test(t.id, "synthetic".to_string(), now_unix() - 3 * 86_400, None, None)
        .await
        .unwrap();

    // Non-loopback, non-fleet, header-less, authenticated, days later.
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "203.0.113.41")])
        .await
        .expect("whoami");

    let t = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();
    assert_eq!(t.origin, "synthetic");
    no_promotion_rows(&server).await;
    assert_eq!(healthz(&server.base_url).await["synthetic_promotion_refused"], json!(1));

    // A loopback call is not a qualifying one: the counter holds.
    McpClient::with_bearer(&server.base_url, &key).tools_call("host.whoami", json!({})).await.expect("whoami");
    assert_eq!(healthz(&server.base_url).await["synthetic_promotion_refused"], json!(1));
}
