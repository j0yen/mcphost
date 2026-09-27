//! PRD-mcphost-enterprise-managed-auth
//! AC3 (P0) — Given assertions with a wrong signature, `aud` of another
//! server, `exp` in the past, a reused `jti`, or an issuer acme never
//! trusted, When each is presented, Then each returns `invalid_grant` with
//! a distinct `error_description`, the audit row names the issuer and
//! reason, and no assertion text is stored or logged.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, priv_pem_2};

use crate::assertion;
use assertion::{fresh_jti, now_unix, sign_assertion};

use serde_json::{Value, json};
use std::collections::HashSet;

async fn post_token(http: &reqwest::Client, base_url: &str, assertion_jwt: &str, resource: &str) -> Value {
    http.post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion_jwt),
            ("client_id", "claude-enterprise"),
            ("resource", resource),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response")
}

#[tokio::test]
async fn each_failure_reason_is_distinct_invalid_grant_and_audited() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Corp").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://idp.acme-test.example.com";
    key_client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": format!("{}/jwks", jwks.uri()),
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect("trusted_issuer_set must succeed");

    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    let http = reqwest::Client::new();
    let mut descriptions: HashSet<String> = HashSet::new();

    // 1. Wrong signature: header names KID_1 (the published key) but the
    // assertion is actually signed with an unrelated key.
    let now = now_unix();
    let wrong_sig_claims = json!({
        "iss": issuer, "aud": server.base_url, "sub": "okta|wrongsig",
        "iat": now, "exp": now + 300, "jti": fresh_jti(),
    });
    let wrong_sig = sign_assertion(KID_1, priv_pem_2(), &wrong_sig_claims);
    let resp = post_token(&http, &server.base_url, &wrong_sig, &resource).await;
    assert_eq!(resp["error"], json!("invalid_grant"), "{resp}");
    descriptions.insert(resp["error_description"].as_str().expect("error_description").to_string());

    // 2. aud of another server.
    let wrong_aud_claims = json!({
        "iss": issuer, "aud": "https://not-mcphost.example.com", "sub": "okta|wrongaud",
        "iat": now, "exp": now + 300, "jti": fresh_jti(),
    });
    let wrong_aud = sign_assertion(KID_1, priv_pem_1(), &wrong_aud_claims);
    let resp = post_token(&http, &server.base_url, &wrong_aud, &resource).await;
    assert_eq!(resp["error"], json!("invalid_grant"), "{resp}");
    descriptions.insert(resp["error_description"].as_str().expect("error_description").to_string());

    // 3. exp in the past.
    let expired_claims = json!({
        "iss": issuer, "aud": server.base_url, "sub": "okta|expired",
        "iat": now - 1000, "exp": now - 900, "jti": fresh_jti(),
    });
    let expired = sign_assertion(KID_1, priv_pem_1(), &expired_claims);
    let resp = post_token(&http, &server.base_url, &expired, &resource).await;
    assert_eq!(resp["error"], json!("invalid_grant"), "{resp}");
    descriptions.insert(resp["error_description"].as_str().expect("error_description").to_string());

    // 4. Reused jti: a fresh, otherwise-valid assertion works once, then
    // presenting the identical assertion again is a replay.
    let jti = fresh_jti();
    let replay_claims = json!({
        "iss": issuer, "aud": server.base_url, "sub": "okta|replay",
        "iat": now, "exp": now + 300, "jti": jti,
    });
    let replay = sign_assertion(KID_1, priv_pem_1(), &replay_claims);
    let first = post_token(&http, &server.base_url, &replay, &resource).await;
    assert!(first["access_token"].as_str().is_some(), "first use must mint a token: {first}");
    let resp = post_token(&http, &server.base_url, &replay, &resource).await;
    assert_eq!(resp["error"], json!("invalid_grant"), "{resp}");
    descriptions.insert(resp["error_description"].as_str().expect("error_description").to_string());

    // 5. Issuer acme never trusted.
    let untrusted_issuer = "https://untrusted.example.com";
    let untrusted_claims = json!({
        "iss": untrusted_issuer, "aud": server.base_url, "sub": "okta|untrusted",
        "iat": now, "exp": now + 300, "jti": fresh_jti(),
    });
    let untrusted = sign_assertion(KID_1, priv_pem_1(), &untrusted_claims);
    let resp = post_token(&http, &server.base_url, &untrusted, &resource).await;
    assert_eq!(resp["error"], json!("invalid_grant"), "{resp}");
    descriptions.insert(resp["error_description"].as_str().expect("error_description").to_string());

    assert_eq!(descriptions.len(), 5, "every failure reason must have its own error_description: {descriptions:?}");

    // The audit row names the issuer and the reason -- no assertion text.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.oauth.trusted_issuers", json!({}))
        .await
        .expect("admin.oauth.trusted_issuers must succeed");
    let listed = common::extract_structured(&result);
    let dump = listed.to_string();
    for signed in [&wrong_sig, &wrong_aud, &expired, &replay, &untrusted] {
        assert!(!dump.contains(signed.as_str()), "admin surface must never carry assertion text: {dump}");
    }
    let issuers = listed["issuers"].as_array().expect("issuers array");
    let row = issuers
        .iter()
        .find(|r| r["issuer"] == json!(issuer))
        .unwrap_or_else(|| panic!("trusted issuer {issuer} must be listed: {issuers:?}"));
    let rejections = row["rejections"].as_object().expect("rejections object");
    for reason in ["bad_signature", "wrong_audience", "expired", "replay"] {
        assert!(
            rejections.get(reason).and_then(Value::as_i64).unwrap_or(0) >= 1,
            "rejections must name reason {reason} for issuer {issuer}: {rejections:?}"
        );
    }
}
