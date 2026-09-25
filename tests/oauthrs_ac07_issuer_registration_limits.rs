//! PRD-mcphost-oauth-resource-server
//! AC7 (P0) — Given tenant T1 registered issuer I, When tenant T2 calls
//! `host.oauth.issuer_set` with the same issuer, Then it is rejected as
//! already registered; T1 may register up to 3 and the 4th is rejected.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn issuer_is_single_tenant_and_capped_at_three() {
    let server = TestServer::start().await;
    let (_ns1, key1) = signup(&server.base_url, "T1").await;
    let (_ns2, key2) = signup(&server.base_url, "T2").await;
    let t1 = McpClient::with_bearer(&server.base_url, &key1);
    let t2 = McpClient::with_bearer(&server.base_url, &key2);

    let issuer = "https://issuer.example.com";
    t1.tools_call(
        "host.oauth.issuer_set",
        json!({"issuer": issuer, "audience": "aud", "jwks_url": "http://127.0.0.1:1/jwks"}),
    )
    .await
    .expect("T1 registers I first");

    let err = t2
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": "aud", "jwks_url": "http://127.0.0.1:1/jwks"}),
        )
        .await
        .expect_err("T2 must be refused: I already belongs to T1");
    assert_eq!(err.error_code.as_deref(), Some("issuer_already_registered"));

    // T1 re-registering its own issuer is idempotent, not a conflict.
    t1.tools_call(
        "host.oauth.issuer_set",
        json!({"issuer": issuer, "audience": "aud2", "jwks_url": "http://127.0.0.1:1/jwks"}),
    )
    .await
    .expect("T1 re-registering its own issuer must succeed");

    // T1 fills its quota: issuer above plus 2 more = 3 total.
    for n in 0..2 {
        t1.tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": format!("https://issuer{n}.example.com"),
                "audience": "aud",
                "jwks_url": "http://127.0.0.1:1/jwks",
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("T1 issuer #{n} within quota must succeed: {} {}", e.code, e.message));
    }

    let err = t1
        .tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": "https://issuer-fourth.example.com",
                "audience": "aud",
                "jwks_url": "http://127.0.0.1:1/jwks",
            }),
        )
        .await
        .expect_err("T1's 4th distinct issuer must be refused");
    assert_eq!(err.error_code.as_deref(), Some("issuer_quota_exceeded"));
}
