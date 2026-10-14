//! Hotfix: prod's root `/.well-known/oauth-protected-resource` listed the
//! placeholder `toolconf-probe` because `host.oauth.issuer_set` stored any
//! string and the root document unions every tenant's issuers. An issuer or
//! jwks_url that is not an https URL is now refused on write.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

async fn root_servers(base_url: &str) -> Value {
    let body: Value = reqwest::get(format!("{base_url}/.well-known/oauth-protected-resource"))
        .await
        .expect("GET well-known")
        .json()
        .await
        .expect("parse metadata JSON");
    body["authorization_servers"].clone()
}

#[tokio::test]
async fn non_https_issuer_is_refused_and_root_document_stays_clean() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Probe Corp").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let before = root_servers(&server.base_url).await;

    let err = client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "toolconf-probe", "audience": "aud", "jwks_url": "https://issuer.example.com/jwks"}),
        )
        .await
        .expect_err("a non-URL issuer must be refused");
    assert!(
        err.message.contains("issuer must be an https:// URL (got \"toolconf-probe\")"),
        "unexpected message: {}",
        err.message
    );

    assert_eq!(root_servers(&server.base_url).await, before);
    assert!(!before.to_string().contains("toolconf-probe"));
}

#[tokio::test]
async fn trusted_issuer_set_refuses_http_issuer() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Acme Corp").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": "http://example.com",
                "jwks_url": "https://example.com/jwks",
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect_err("an http:// issuer must be refused");
    assert!(err.message.contains("issuer must be an https:// URL"), "unexpected message: {}", err.message);
}

#[tokio::test]
async fn non_https_jwks_url_is_refused_and_valid_issuer_succeeds() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Acme Corp").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "https://issuer.example.com", "audience": "aud", "jwks_url": "http://example.com/jwks"}),
        )
        .await
        .expect_err("a non-loopback http:// jwks_url must be refused");
    assert!(err.message.contains("jwks_url must be an https:// URL"), "unexpected message: {}", err.message);

    client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "https://issuer.example.com", "audience": "aud", "jwks_url": "https://issuer.example.com/jwks"}),
        )
        .await
        .expect("a valid https issuer must still register");
}
