//! PRD-mcphost-external-class-positive-evidence
//! AC2 — Given an `unattributed` tenant, When its claim e-mail is verified
//! (`owner_verified_at` set), Then `origin='external'`,
//! `origin_detail='claimed-email'` and exactly one `admin_audit` row
//! `origin_promoted` exists even when two verifications race.

use crate::common;
use common::TestServer;
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after.split_whitespace().next().expect("code token")
}

async fn promoted_rows(server: &TestServer, namespace: &str) -> Vec<mcphost::db::AdminAuditRow> {
    server
        .state
        .db
        .list_admin_audit(1000, None)
        .await
        .expect("audit rows")
        .into_iter()
        .filter(|r| r.action == "origin_promoted" && r.target.as_deref() == Some(namespace))
        .collect()
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02_email_claim_promotes_exactly_once() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;

    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Claimer"}),
        "198.51.100.20",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let namespace = result["tenant"].as_str().expect("tenant").to_string();
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let before = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(before.origin, "unattributed");
    assert!(promoted_rows(&server, &namespace).await.is_empty());

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "ac2@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let code = code_from_email_body(&fake.sends()[0].text_body).to_string();
    let verify = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(verify.status(), reqwest::StatusCode::OK);
    // A repeated visit of the (now consumed) link promotes nothing further.
    let _ = http.get(format!("{}/claim/verify/{code}", server.base_url)).send().await;

    let after = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(after.origin, "external");
    assert_eq!(after.origin_detail.as_deref(), Some("claimed-email"));
    let rows = promoted_rows(&server, &namespace).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let detail = rows[0].detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("from=unattributed") && detail.contains("to=external") && detail.contains("detail=claimed-email"),
        "{detail}"
    );

    let health: serde_json::Value = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["tenants_by_origin"]["external"], json!(1), "{health:?}");
    assert_eq!(health["external_by_evidence"]["claimed"], json!(1), "{health:?}");
    assert_eq!(health["tenants_confirmed"], json!(1), "{health:?}");
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02_racing_verifications_yield_one_audit_row() {
    let server = TestServer::start().await;
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Racer"}),
        "198.51.100.21",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let namespace = result["tenant"].as_str().expect("tenant").to_string();
    let tenant = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();

    let db = server.state.db.clone();
    let (a, b) = tokio::join!(
        db.verify_owner(tenant.id, "dns".to_string(), "racer-a.example".to_string()),
        db.verify_owner(tenant.id, "dns".to_string(), "racer-b.example".to_string()),
    );
    a.expect("first verification");
    b.expect("second verification");

    let after = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(after.origin, "external");
    assert_eq!(after.origin_detail.as_deref(), Some("claimed-email"));
    assert_eq!(promoted_rows(&server, &namespace).await.len(), 1);
}
