//! PRD-mcphost-oauth-metadata-lists-hosted-as
//! AC3 (P0) — Given `host.oauth.trusted_issuer_set` with issuer
//! `toolconf-probe`, `ftp://x`, or `https://`, When called, Then the error
//! code is `issuer_url_invalid` and no row is written; Given
//! `https://idp.example.com` or `http://127.0.0.1:8080`, Then the row is
//! written. `issuer_set` and `provider_set` share the validator (R2).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn trusted_issuer_set_refuses_non_urls_and_writes_valid_ones() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Trusted Issuer Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant row");

    for bad in ["toolconf-probe", "ftp://x", "https://"] {
        let err = client
            .tools_call(
                "host.oauth.trusted_issuer_set",
                json!({"issuer": bad, "jwks_url": "https://idp.example.com/jwks", "client_id": "c"}),
            )
            .await
            .expect_err("a non-URL issuer must be refused");
        assert_eq!(err.error_code.as_deref(), Some("issuer_url_invalid"), "issuer {bad:?}: {}", err.message);
    }
    let rows = server.state.db.list_trusted_issuers_by_tenant(tenant.id).await.unwrap();
    assert!(rows.is_empty(), "no row may be written for a refused issuer: {} rows", rows.len());

    for good in ["https://idp.example.com", "http://127.0.0.1:8080"] {
        client
            .tools_call(
                "host.oauth.trusted_issuer_set",
                json!({"issuer": good, "jwks_url": "https://idp.example.com/jwks", "client_id": "c"}),
            )
            .await
            .unwrap_or_else(|e| panic!("issuer {good:?} must be accepted: {}", e.message));
    }
    let rows = server.state.db.list_trusted_issuers_by_tenant(tenant.id).await.unwrap();
    let mut written: Vec<String> = rows.into_iter().map(|r| r.issuer).collect();
    written.sort();
    assert_eq!(written, vec!["http://127.0.0.1:8080".to_string(), "https://idp.example.com".to_string()]);
}

#[tokio::test]
async fn issuer_set_and_provider_set_share_the_validator() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Shared Validator Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant row");

    let err = client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "ftp://x", "audience": "a", "jwks_url": "https://idp.example.com/jwks"}),
        )
        .await
        .expect_err("issuer_set must refuse ftp://x");
    assert_eq!(err.error_code.as_deref(), Some("issuer_url_invalid"), "{}", err.message);
    assert!(server.state.db.list_oauth_issuers_by_tenant(tenant.id).await.unwrap().is_empty());

    let err = client
        .tools_call(
            "host.oauth.provider_set",
            json!({"issuer": "toolconf-probe", "client_id": "c", "client_secret": "s"}),
        )
        .await
        .expect_err("provider_set must refuse toolconf-probe");
    assert_eq!(err.error_code.as_deref(), Some("issuer_url_invalid"), "{}", err.message);
}
