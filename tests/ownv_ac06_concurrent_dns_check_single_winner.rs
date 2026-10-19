//! PRD-mcphost-owner-verification-paths
//! AC6 (P0) — Given two concurrent `dns.check` calls for the same tenant,
//! When both match, Then exactly one writes and one `tenant_audit` row
//! `claim.dns` exists.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::verify::{DnsResolver, OwnerVerify};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct FakeTxt(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl DnsResolver for FakeTxt {
    async fn txt(&self, _name: &str) -> Result<Vec<String>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn ownv_ac06_two_matching_checks_write_once() {
    let resolver = Arc::new(FakeTxt(Mutex::new(Vec::new())));
    let server = TestServer::start_with_owner_verify(OwnerVerify { github: None, resolver: resolver.clone() }).await;
    let (ns, key) = signup(&server.base_url, "Ownv AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A mismatch first: the record is not published yet, so nothing verifies.
    let miss = client.tools_call("host.claim.dns.check", json!({"domain": "acme.dev"})).await.expect_err("miss");
    assert_eq!(miss.error_code.as_deref(), Some("dns_record_not_found"), "{}", miss.message);
    assert_eq!(miss.data["seen"], json!([]));
    let expected = miss.data["expected"].as_str().expect("expected").to_string();

    let record = extract_structured(&client.tools_call("host.claim.dns.start", json!({"domain": "acme.dev"})).await.expect("dns"));
    assert_eq!(record["value"], expected, "dns.check and host.claim.dns.start name the same token");
    assert_eq!(record["type"], "TXT");
    resolver.0.lock().unwrap().push(expected);

    let (a, b) = tokio::join!(
        client.tools_call("host.claim.dns.check", json!({"domain": "acme.dev"})),
        client.tools_call("host.claim.dns.check", json!({"domain": "acme.dev"})),
    );
    let outcomes = [a, b];
    let winners = outcomes.iter().filter(|r| r.is_ok()).count();
    assert_eq!(winners, 1, "exactly one call writes: {outcomes:?}");
    let loser = outcomes.iter().find_map(|r| r.as_ref().err()).expect("one loser");
    assert_eq!(loser.error_code.as_deref(), Some("already_verified"), "{}", loser.message);
    assert_eq!(loser.data["method"], "dns");

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();
    assert!(tenant.owner_verified_at.is_some());
    let (method, handle) = server.state.db.owner_provenance(tenant.id).await.unwrap();
    assert_eq!((method.as_deref(), handle.as_deref()), (Some("dns"), Some("acme.dev")));
    let audit = server.state.db.list_tenant_audit_for_subject(tenant.id, "owner".to_string(), None, 10).await.unwrap();
    let claims: Vec<&str> = audit.iter().map(|a| a.1.as_str()).filter(|a| *a == "claim.dns").collect();
    assert_eq!(claims.len(), 1, "one claim.dns audit row: {audit:?}");
}
