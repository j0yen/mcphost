//! PRD-mcphost-owner-verification-paths
//! AC5 (P0) — Given a tenant already verified by email, When it runs
//! `host.claim.dns.check` with a matching record, Then `already_verified
//! {method: "email"}` and `owner_method` is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::verify::{DnsResolver, OwnerVerify};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// A resolver that answers every name with the strings it was given.
struct FakeTxt(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl DnsResolver for FakeTxt {
    async fn txt(&self, _name: &str) -> Result<Vec<String>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    body[idx + "/claim/verify/".len()..].split_whitespace().next().expect("code")
}

#[tokio::test]
async fn ownv_ac05_dns_check_on_an_email_verified_tenant_is_already_verified() {
    let resolver = Arc::new(FakeTxt(Mutex::new(Vec::new())));
    let fake_email = Arc::new(mcphost::email::FakeEmailClient::new());
    let ov = OwnerVerify { github: None, resolver: resolver.clone() };
    let server = TestServer::start_with_owner_verify_and_email(ov, fake_email.clone()).await;

    let signup = extract_structured(
        &McpClient::new(&server.base_url).tools_call("signup", json!({"name": "Ownv AC5"})).await.expect("signup"),
    );
    let namespace = signup["tenant"].as_str().unwrap().to_string();
    let claim_token = signup["claim_url"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
    let client = McpClient::with_bearer(&server.base_url, signup["key"].as_str().unwrap());

    // The record is published and matches: dns.check would verify an unverified tenant.
    let record = extract_structured(&client.tools_call("host.claim.dns", json!({"domain": "acme.dev"})).await.expect("dns"));
    assert_eq!(record["record"], "_mcphost.acme.dev");
    resolver.0.lock().unwrap().push(record["value"].as_str().unwrap().to_string());

    // Verify by email through the real claim flow.
    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "owner@acme.dev")])
        .send()
        .await
        .expect("POST claim");
    let code = code_from_email_body(&fake_email.sends()[0].text_body).to_string();
    let verified = http.get(format!("{}/claim/verify/{code}", server.base_url)).send().await.expect("verify");
    assert_eq!(verified.status(), 200);

    let before = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(server.state.db.owner_provenance(before.id).await.unwrap().0.as_deref(), Some("email"));

    let err = client
        .tools_call("host.claim.dns.check", json!({"domain": "acme.dev"}))
        .await
        .expect_err("already_verified");
    assert_eq!(err.error_code.as_deref(), Some("already_verified"), "{}", err.message);
    assert_eq!(err.data["method"], "email");

    let after = server.state.db.find_tenant_by_namespace(namespace).await.unwrap().unwrap();
    assert_eq!(after.owner_verified_at, before.owner_verified_at, "verified_at must not move");
    let (method, handle) = server.state.db.owner_provenance(after.id).await.unwrap();
    assert_eq!(method.as_deref(), Some("email"), "owner_method unchanged");
    assert_eq!(handle, None, "no handle written by the refused path");
    let audit = server.state.db.list_tenant_audit_for_subject(after.id, "owner".to_string(), None, 10).await.unwrap();
    assert_eq!(audit.iter().map(|a| a.1.as_str()).collect::<Vec<_>>(), vec!["claim.email"]);

    // R5: whoami and admin.tenants carry the method.
    let me = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["owner"]["method"], "email");
    assert_eq!(me["owner_verified"], true);
}

#[tokio::test]
async fn ownv_ac05_admin_tenants_row_carries_owner_method() {
    let server = TestServer::start().await;
    let (ns, _key) = common::signup(&server.base_url, "Ownv AC5 Admin").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let row_of = |listing: &serde_json::Value| {
        listing["tenants"].as_array().unwrap().iter().find(|t| t["tenant"] == json!(ns)).unwrap().clone()
    };
    let before = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    assert_eq!(row_of(&before)["owner_method"], serde_json::Value::Null);

    server.state.db.verify_owner(tenant.id, "dns".to_string(), "acme.dev".to_string()).await.unwrap();
    let after = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    assert_eq!(row_of(&after)["owner_method"], "dns");
    assert_eq!(row_of(&after)["owner_verified"], true);
}
