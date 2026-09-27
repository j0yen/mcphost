//! PRD-mcphost-oauth-client-policy
//! AC5 (P0) — Given three grants and one pending client, When
//! `host.oauth.revoke_all` runs, Then all four are gone, every access
//! token fails within 60 s, and one audit row per revoked grant exists.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

async fn register_dcr_client(http: &reqwest::Client, base_url: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

/// Runs a full authorize -> code -> token exchange for a fresh DCR client,
/// returning its access token.
async fn complete_grant(http: &reqwest::Client, base_url: &str, key: &str, resource: &str, tag: &str) -> String {
    let client_id = register_dcr_client(http, base_url).await;
    let verifier = format!("verifier-for-{tag}-at-least-43-characters-long-ok");
    let challenge = code_challenge_for(&verifier);
    let consent = http
        .post(format!("{base_url}/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", tag),
            ("scope", "mcp"),
            ("resource", resource),
            ("tenant_key", key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent.status().is_redirection(), "{tag} must reach consent: {}", consent.status());
    let location = consent.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_base, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").expect("code present").to_string();

    let token_resp: Value = http
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    token_resp["access_token"].as_str().expect("access_token").to_string()
}

#[tokio::test]
async fn revoke_all_clears_grants_pending_and_access_tokens_with_one_audit_row_each() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RevokeAll Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    let access_1 = complete_grant(&http, &server.base_url, &key, &resource, "grant-1").await;
    let access_2 = complete_grant(&http, &server.base_url, &key, &resource, "grant-2").await;
    let access_3 = complete_grant(&http, &server.base_url, &key, &resource, "grant-3").await;

    let grants_before = tenant_client.tools_call("host.oauth.grants", json!({})).await.expect("host.oauth.grants");
    assert_eq!(
        common::extract_structured(&grants_before)["grants"].as_array().unwrap().len(),
        3,
        "three live grants must exist before revoke_all"
    );

    // A fourth, still-pending client -- switching to approve mode doesn't
    // touch the three grants already minted under the prior (default any)
    // policy.
    tenant_client
        .tools_call("host.oauth.policy_set", json!({"clients": "approve"}))
        .await
        .expect("policy_set approve");
    let pending_client = register_dcr_client(&http, &server.base_url).await;
    let pending_verifier = "verifier-for-pending-client-at-least-43-chars-ok";
    let pending_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", pending_client.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", code_challenge_for(pending_verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", "pending"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(pending_resp.headers().get("location").is_none(), "the fourth client must be held pending");
    let pending_list = tenant_client.tools_call("host.oauth.pending", json!({})).await.expect("host.oauth.pending");
    assert_eq!(common::extract_structured(&pending_list)["pending"].as_array().unwrap().len(), 1);

    // Every access token works before revoke_all.
    for access in [&access_1, &access_2, &access_3] {
        let bearer = McpClient::with_bearer(&server.base_url, access);
        bearer.tools_call("host.whoami", json!({})).await.expect("access token must work before revoke_all");
    }

    let revoked = tenant_client.tools_call("host.oauth.revoke_all", json!({})).await.expect("host.oauth.revoke_all");
    let revoked_structured = common::extract_structured(&revoked);
    assert_eq!(revoked_structured["revoked_grants"], json!(3));
    assert_eq!(revoked_structured["revoked_pending"], json!(1));

    // All four are gone.
    let grants_after = tenant_client.tools_call("host.oauth.grants", json!({})).await.expect("host.oauth.grants");
    assert!(common::extract_structured(&grants_after)["grants"].as_array().unwrap().is_empty());
    let pending_after = tenant_client.tools_call("host.oauth.pending", json!({})).await.expect("host.oauth.pending");
    assert!(common::extract_structured(&pending_after)["pending"].as_array().unwrap().is_empty());

    // Every access token fails immediately (well within 60s).
    for access in [&access_1, &access_2, &access_3] {
        let bearer = McpClient::with_bearer(&server.base_url, access);
        let err = bearer
            .tools_call("host.whoami", json!({}))
            .await
            .expect_err("access token must fail after revoke_all");
        assert_ne!(err.code, 0);
    }

    // One audit row per revoked grant.
    let audit = tenant_client.tools_call("host.oauth.audit", json!({"event": "revoke"})).await.expect("host.oauth.audit");
    let revoke_events = common::extract_structured(&audit)["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(revoke_events.len(), 3, "exactly one audit row per revoked grant: {revoke_events:?}");
}
