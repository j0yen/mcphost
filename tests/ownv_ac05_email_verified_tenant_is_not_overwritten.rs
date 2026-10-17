//! PRD-mcphost-owner-verification-paths AC5 (P0) -- Given a tenant already
//! verified by email, When it runs `host.claim.dns.check` with a matching
//! record, Then `already_verified {method: "email"}` and `owner_method` is
//! unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn code_from_email(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    body[idx + "/claim/verify/".len()..]
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .next()
        .expect("code")
}

#[tokio::test]
async fn ownv_ac05() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);
    let signed = extract_structured(&anon.tools_call("signup", json!({"name": "Ownv AC5 Tenant"})).await.expect("signup"));
    let key = signed["key"].as_str().expect("key").to_string();
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A DNS claim is started and its record is live in the resolver...
    let domain = "ownv-ac05.example";
    let started = extract_structured(&client.tools_call("host.claim.dns", json!({"domain": domain})).await.expect("claim.dns"));
    mcphost::verify::fixture_resolver().set(
        started["record"].as_str().expect("record"),
        vec![started["value"].as_str().expect("value").to_string()],
    );

    // ...but the tenant verifies by email first.
    let sent = extract_structured(&client.tools_call("host.claim.start", json!({"email": "owner@example.org"})).await.expect("claim.start"));
    assert_eq!(sent["sent"], json!(true), "{sent}");
    let sends = fake.sends();
    assert_eq!(sends.len(), 1);
    let resp = reqwest::get(format!("{}/claim/verify/{}", server.base_url, code_from_email(&sends[0].text_body)))
        .await
        .expect("verify");
    assert!(resp.status().is_success(), "{}", resp.status());
    let me = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["owner"]["method"], json!("email"), "{me}");

    let err = client
        .tools_call("host.claim.dns.check", json!({"domain": domain}))
        .await
        .expect_err("already verified");
    assert_eq!(err.error_code.as_deref(), Some("already_verified"), "{err:?}");
    assert_eq!(err.data["method"], json!("email"), "{err:?}");

    let after = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(after["owner"]["method"], json!("email"), "{after}");
    assert_eq!(after["owner"]["verified_at"], me["owner"]["verified_at"], "{after}");
    let tenant = server.state.db.find_tenant_by_namespace(signed["namespace"].as_str().expect("ns").to_string()).await.expect("db").expect("tenant");
    assert_eq!(server.state.db.count_tenant_audit_for_test(tenant.id, "claim.dns".to_string()).await.expect("audit"), 0);
    assert_eq!(server.state.db.count_tenant_audit_for_test(tenant.id, "claim.email".to_string()).await.expect("audit"), 1);
}
