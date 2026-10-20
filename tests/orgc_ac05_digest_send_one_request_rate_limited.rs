//! PRD-mcphost-org-cost-attribution
//! AC5 — Given a verified owner, When `host.org.digest.send` runs against a
//! stub email API, Then one request is made with the org name in the
//! subject, the 7d rows in the body, the opt-out line, and no key or tenant
//! secret anywhere in the payload; a second call within 1 h answers
//! `rate_limited`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::db::OperatorProof;
use mcphost::email::FakeEmailClient;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn orgc_ac05() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create");
    owner
        .tools_call("host.secret.set", json!({"name": "api", "value": "tenant-secret-value-123"}))
        .await
        .expect("secret");
    let owner_id = server.state.db.find_tenant_by_namespace(owner_ns.clone()).await.expect("lookup").expect("owner").id;
    for _ in 0..3 {
        server
            .state
            .db
            .record_call(owner_id, "t".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("record_call");
    }

    // An unverified owner is refused and nothing is sent.
    let err = owner.tools_call("host.org.digest.send", json!({})).await.expect_err("unverified");
    assert_eq!(err.error_code.as_deref(), Some("owner_unverified"), "{err:?}");
    assert_eq!(fake.send_count(), 0);

    server
        .state
        .db
        .verify_owner_as(
            owner_id,
            "email".to_string(),
            "owner@example.com".to_string(),
            Some(OperatorProof { email: "owner@example.com".to_string(), actor: "test".to_string() }),
        )
        .await
        .expect("verify");

    let sent = extract_structured(&owner.tools_call("host.org.digest.send", json!({})).await.expect("send"));
    assert_eq!(sent["sent"], json!(true), "{sent}");
    let mails = fake.sends();
    assert_eq!(mails.len(), 1, "exactly one request: {mails:?}");
    let mail = &mails[0];
    assert!(mail.subject.contains("acme"), "org name in subject: {}", mail.subject);
    assert!(mail.text_body.contains(&owner_ns) && mail.text_body.contains("3 calls"), "7d rows: {}", mail.text_body);
    assert!(mail.text_body.contains(mcphost::org_digest::OPT_OUT_LINE), "opt-out line: {}", mail.text_body);
    let payload = format!("{mail:?}");
    for secret in [owner_key.as_str(), "tenant-secret-value-123", "test-email-key"] {
        assert!(!payload.contains(secret), "payload must not carry {secret}");
    }

    let err = owner.tools_call("host.org.digest.send", json!({})).await.expect_err("second send");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"), "{err:?}");
    assert_eq!(fake.send_count(), 1, "the refused call sent nothing");

    // The opt-out verb is owner-only, audits, and flips the flag the weekly job reads.
    let off = extract_structured(&owner.tools_call("host.org.digest.set", json!({"enabled": false})).await.expect("set"));
    assert_eq!(off["digest_enabled"], json!(false), "{off}");
}
