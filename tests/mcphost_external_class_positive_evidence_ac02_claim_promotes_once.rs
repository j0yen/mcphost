//! PRD-mcphost-external-class-positive-evidence
//! AC2 — Given an `unattributed` tenant, When its claim e-mail is verified,
//! Then `origin='external'`, `origin_detail='claimed-email'` and exactly one
//! `admin_audit` row `origin_promoted` exists even when promotions race,
//! because promotion is one `UPDATE ... WHERE origin='unattributed'` and the
//! audit row is written only when it changed one row.

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
        .expect("audit")
        .into_iter()
        .filter(|r| r.action == "origin_promoted" && r.target.as_deref() == Some(namespace))
        .collect()
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;

    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Tenant"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let namespace = result["tenant"].as_str().expect("tenant").to_string();
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let tenant = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(tenant.origin, "unattributed");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "ac2@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let code = code_from_email_body(&fake.sends()[0].text_body).to_string();
    // Two verifications of the same code race; one wins.
    let url = format!("{}/claim/verify/{code}", server.base_url);
    let (a, b) = tokio::join!(http.get(&url).send(), http.get(&url).send());
    a.expect("verify a");
    b.expect("verify b");

    let tenant = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert!(tenant.owner_verified_at.is_some(), "claim must verify");
    assert_eq!(tenant.origin, "external");
    assert_eq!(tenant.origin_detail.as_deref(), Some("claimed-email"));

    // Direct racing promotions on the already-promoted tenant change nothing.
    let db = server.state.db.clone();
    let (p1, p2) = tokio::join!(
        db.promote_origin(tenant.id, mcphost::db::OriginEvidence::ClaimedEmail),
        db.promote_origin(tenant.id, mcphost::db::OriginEvidence::ClaimedEmail),
    );
    assert_eq!(p1.unwrap(), mcphost::db::PromoteOutcome::Unchanged);
    assert_eq!(p2.unwrap(), mcphost::db::PromoteOutcome::Unchanged);

    let rows = promoted_rows(&server, &namespace).await;
    assert_eq!(rows.len(), 1, "exactly one origin_promoted row: {rows:?}");
    let detail = rows[0].detail.as_deref().unwrap();
    assert!(
        detail.contains("from=unattributed") && detail.contains("to=external") && detail.contains("detail=claimed-email"),
        "{detail}"
    );
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac02_racing_promotions_write_one_row() {
    let server = TestServer::start().await;
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Race"}),
        "8.8.4.4",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("signup");
    let namespace = result["tenant"].as_str().unwrap().to_string();
    let tenant = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .set_claim_timing_for_test(tenant.id, "unattributed".to_string(), now, Some(now), None)
        .await
        .unwrap();

    let db = server.state.db.clone();
    let (p1, p2) = tokio::join!(
        db.promote_origin(tenant.id, mcphost::db::OriginEvidence::ClaimedEmail),
        db.promote_origin(tenant.id, mcphost::db::OriginEvidence::ClaimedEmail),
    );
    let outcomes = [p1.unwrap(), p2.unwrap()];
    assert_eq!(
        outcomes.iter().filter(|o| **o == mcphost::db::PromoteOutcome::Promoted).count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(promoted_rows(&server, &namespace).await.len(), 1);
}
