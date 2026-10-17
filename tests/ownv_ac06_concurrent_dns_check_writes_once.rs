//! PRD-mcphost-owner-verification-paths AC6 (P0) -- Given two concurrent
//! `dns.check` calls for the same tenant, When both match, Then exactly one
//! writes and one `tenant_audit` row `claim.dns` exists.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ownv_ac06() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ownv AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let other = McpClient::with_bearer(&server.base_url, &key);

    let domain = "ownv-ac06.example";
    let started = extract_structured(&client.tools_call("host.claim.dns", json!({"domain": domain})).await.expect("claim.dns"));
    assert_eq!(started["type"], json!("TXT"), "{started}");
    assert_eq!(started["record"], json!(format!("_mcphost.{domain}")), "{started}");
    let value = started["value"].as_str().expect("value").to_string();
    assert!(value.starts_with("mcphost-verify="), "{started}");
    assert_eq!(value.len(), "mcphost-verify=".len() + 64, "32-byte hex token: {started}");

    // A wrong record is a mismatch that names what was expected and seen.
    let fixture = mcphost::verify::fixture_resolver();
    fixture.set(&format!("_mcphost.{domain}"), vec!["mcphost-verify=wrong".to_string()]);
    let miss = client.tools_call("host.claim.dns.check", json!({"domain": domain})).await.expect_err("mismatch");
    assert_eq!(miss.error_code.as_deref(), Some("dns_record_not_found"), "{miss:?}");
    assert_eq!(miss.data["expected"], json!(value), "{miss:?}");
    assert_eq!(miss.data["seen"], json!(["mcphost-verify=wrong"]), "{miss:?}");

    fixture.set(&format!("_mcphost.{domain}"), vec![value.clone()]);
    let (a, b) = tokio::join!(
        client.tools_call("host.claim.dns.check", json!({"domain": domain})),
        other.tools_call("host.claim.dns.check", json!({"domain": domain})),
    );
    let wins = [&a, &b].iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1, "exactly one writer: {a:?} / {b:?}");
    let loser = [a, b].into_iter().find_map(Result::err).expect("one loser");
    assert_eq!(loser.error_code.as_deref(), Some("already_verified"), "{loser:?}");
    assert_eq!(loser.data["method"], json!("dns"), "{loser:?}");

    let me = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["owner"]["method"], json!("dns"), "{me}");
    assert_eq!(me["owner"]["handle"], json!(domain), "{me}");
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db").expect("tenant");
    assert_eq!(server.state.db.count_tenant_audit_for_test(tenant.id, "claim.dns".to_string()).await.expect("audit"), 1);
}
