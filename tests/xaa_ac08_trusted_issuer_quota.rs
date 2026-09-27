//! PRD-mcphost-enterprise-managed-auth
//! AC8 (P0) — Given a fifth trusted issuer, When registered, Then it is
//! rejected with `quota_trusted_issuers`.

use crate::common;
use common::{McpClient, TestServer, signup};

use serde_json::json;

#[tokio::test]
async fn a_fifth_distinct_trusted_issuer_is_rejected_with_quota_error() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Acme Corp").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    for i in 0..4 {
        key_client
            .tools_call(
                "host.oauth.trusted_issuer_set",
                json!({
                    "issuer": format!("https://idp{i}.acme-test.example.com"),
                    "jwks_url": format!("https://idp{i}.acme-test.example.com/jwks"),
                    "client_id": "claude-enterprise",
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("issuer {i} must register: {} {}", e.code, e.message));
    }

    let err = key_client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": "https://idp4.acme-test.example.com",
                "jwks_url": "https://idp4.acme-test.example.com/jwks",
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect_err("a 5th distinct trusted issuer must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("quota_trusted_issuers"), "{err:?}");
}
