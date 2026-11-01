//! PRD-mcphost-external-class-positive-evidence
//! AC2 (P0) — Given an `unattributed` tenant, When its claim e-mail is
//! verified (`owner_verified_at` set), Then `origin='external'`,
//! `origin_detail='claimed-email'`, and exactly one `admin_audit` row
//! `origin_promoted` (from, to, detail) exists even when two verifications
//! race, because promotion is a single-row `UPDATE ... WHERE
//! origin='unattributed'` and the audit row is written only when it changed
//! one row.

use crate::common;
use common::TestServer;
use mcphost::control::{SignupAttribution, signup};
use mcphost::db::{OriginEvidence, PromoteOutcome};
use serde_json::{Value, json};
use std::sync::Arc;

async fn unattributed_tenant(server: &TestServer, name: &str, ip: &str) -> (i64, String, Value) {
    let result = signup(&server.state, &json!({"name": name}), ip, SignupAttribution::default())
        .await
        .expect("signup");
    let ns = result["tenant"].as_str().expect("tenant").to_string();
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "unattributed");
    (t.id, ns, result)
}

async fn promoted_rows(server: &TestServer) -> Vec<mcphost::db::AdminAuditRow> {
    server
        .state
        .db
        .list_admin_audit(500, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted")
        .collect()
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02_verified_claim_email_promotes() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let (_, ns, result) = unattributed_tenant(&server, "AC2 Claimer", "203.0.113.21").await;
    let claim_url = result["claim_url"].as_str().expect("claim_url");
    let token = claim_url.rsplit('/').next().unwrap();

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "ac2@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let body = &fake.sends()[0].text_body;
    let idx = body.find("/claim/verify/").expect("verify URL");
    let code = body[idx + "/claim/verify/".len()..].split_whitespace().next().unwrap().to_string();

    // Before the verification: still unattributed, no promotion row.
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "unattributed");
    assert!(promoted_rows(&server).await.is_empty());

    let resp = http.get(format!("{}/claim/verify/{code}", server.base_url)).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert!(t.owner_verified_at.is_some());
    assert_eq!(t.origin, "external");
    assert_eq!(t.origin_detail.as_deref(), Some("claimed-email"));

    let rows = promoted_rows(&server).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let detail: Value = serde_json::from_str(rows[0].detail.as_deref().unwrap()).unwrap();
    assert_eq!(detail["from"], json!("unattributed"));
    assert_eq!(detail["to"], json!("external"));
    assert_eq!(detail["detail"], json!("claimed-email"));
    assert_eq!(rows[0].target.as_deref(), Some(ns.as_str()));
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02_racing_verifications_write_one_audit_row() {
    let server = TestServer::start().await;
    let (id, ns, _) = unattributed_tenant(&server, "AC2 Racer", "203.0.113.22").await;

    // Two owner verifications and two direct promotions, all at once.
    let mut tasks = Vec::new();
    for n in 0..2 {
        let db = server.state.db.clone();
        tasks.push(tokio::spawn(async move {
            db.verify_owner(id, "email".to_string(), format!("racer{n}@example.com")).await.map(|_| ())
        }));
    }
    for _ in 0..2 {
        let db = server.state.db.clone();
        tasks.push(tokio::spawn(async move {
            db.promote_origin(id, OriginEvidence::ClaimedEmail).await.map(|_| ())
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }

    let t = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();
    assert_eq!(t.origin, "external");
    assert_eq!(t.origin_detail.as_deref(), Some("claimed-email"));
    assert_eq!(promoted_rows(&server).await.len(), 1);

    // A later promotion attempt changes nothing and writes nothing.
    let again = server.state.db.promote_origin(id, OriginEvidence::ClaimedEmail).await.unwrap();
    assert_eq!(again, PromoteOutcome::NotPromoted);
    assert_eq!(promoted_rows(&server).await.len(), 1);
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02_claim_predicate_is_conjunctive() {
    let server = TestServer::start().await;
    let (id, ns, _) = unattributed_tenant(&server, "AC2 Unverified", "203.0.113.23").await;
    // `owner_verified_at` still NULL: the claim evidence alone promotes nothing.
    let out = server.state.db.promote_origin(id, OriginEvidence::ClaimedEmail).await.unwrap();
    assert_eq!(out, PromoteOutcome::NotPromoted);
    let t = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();
    assert_eq!(t.origin, "unattributed");
    assert!(promoted_rows(&server).await.is_empty());
}
