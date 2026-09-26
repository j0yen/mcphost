//! PRD-mcphost-hosted-authorization-server
//! AC8 (P0) — Given two grants for T, When `host.oauth.grants` runs, Then
//! both appear with `client_name`, `method`, `created_at`, `last_used_at`;
//! When `host.oauth.grant_revoke` removes one, Then its access token
//! fails within 60s and `admin_audit` records the revocation; When `POST
//! /oauth/revoke` receives the other's refresh token, Then that grant is
//! gone too.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use serde_json::{Value, json};

const REDIRECT_URI: &str = "http://127.0.0.1/cb";

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

struct GrantOutcome {
    access_token: String,
    refresh_token: String,
}

async fn complete_grant(
    http: &reqwest::Client,
    server: &TestServer,
    client_id: &str,
    key: &str,
    verifier: &str,
) -> GrantOutcome {
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/mcp", server.base_url);
    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    let location = consent_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").unwrap().to_string();

    let token: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("token exchange")
        .json()
        .await
        .expect("parse token response");
    GrantOutcome {
        access_token: token["access_token"].as_str().unwrap().to_string(),
        refresh_token: token["refresh_token"].as_str().unwrap().to_string(),
    }
}

#[tokio::test]
async fn two_grants_listed_one_revoked_one_oauth_revoked() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Grants Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({
            "application_type": "native",
            "redirect_uris": [REDIRECT_URI],
            "client_name": "Grants Test Client",
        }))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().unwrap().to_string();

    let grant_1 = complete_grant(&http, &server, &client_id, &key, "verifier-one-at-least-43-characters-long").await;

    // Capture grant 1's id unambiguously (a single grant exists so far)
    // before grant 2 is even created, since `host.oauth.grants`'s list
    // order is not something this test should have to assume matches
    // creation order.
    let grants_after_first = key_client
        .tools_call("host.oauth.grants", json!({}))
        .await
        .expect("host.oauth.grants must succeed");
    let grants_after_first = common::extract_structured(&grants_after_first);
    let grants_after_first_arr = grants_after_first["grants"].as_array().expect("grants array");
    assert_eq!(grants_after_first_arr.len(), 1, "grants: {grants_after_first}");
    let grant_1_id = grants_after_first_arr[0]["id"].as_i64().expect("grant id");

    let grant_2 = complete_grant(&http, &server, &client_id, &key, "verifier-two-at-least-43-characters-long").await;

    let grants_result = key_client
        .tools_call("host.oauth.grants", json!({}))
        .await
        .expect("host.oauth.grants must succeed");
    let grants = common::extract_structured(&grants_result);
    let grants_arr = grants["grants"].as_array().expect("grants array");
    assert_eq!(grants_arr.len(), 2, "grants: {grants}");
    for g in grants_arr {
        assert_eq!(g["client_name"], json!("Grants Test Client"), "grant: {g}");
        assert_eq!(g["method"], json!("dcr"), "grant: {g}");
        assert!(g["created_at"].as_i64().is_some(), "grant: {g}");
        assert!(g["last_used_at"].as_i64().is_some(), "grant: {g}");
    }

    let grant_2_id = grants_arr
        .iter()
        .find_map(|g| g["id"].as_i64().filter(|id| *id != grant_1_id))
        .expect("the second, newly created grant's id");

    // Revoke the first grant via the tenant tool.
    key_client
        .tools_call("host.oauth.grant_revoke", json!({"id": grant_1_id}))
        .await
        .expect("host.oauth.grant_revoke must succeed");

    let bearer_1 = McpClient::with_bearer(&server.base_url, &grant_1.access_token);
    let err = bearer_1
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("revoked grant's access token must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_token"), "err: {err:?}");

    // admin_audit records the revocation.
    let admin_client = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let audit_result = admin_client
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log must succeed");
    let audit = common::extract_structured(&audit_result);
    let entries = audit["entries"].as_array().expect("entries array");
    assert!(
        entries.iter().any(|e| {
            e["action"] == json!("oauth_grant_revoke") && e["target"] == json!(grant_1_id.to_string())
        }),
        "admin_audit must record the revocation: {entries:?}"
    );

    // POST /oauth/revoke with the second grant's refresh token removes it too.
    let revoke_resp = http
        .post(format!("{}/oauth/revoke", server.base_url))
        .form(&[("token", grant_2.refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/revoke");
    assert_eq!(revoke_resp.status(), reqwest::StatusCode::OK);

    let grants_after = key_client
        .tools_call("host.oauth.grants", json!({}))
        .await
        .expect("host.oauth.grants must succeed");
    let grants_after = common::extract_structured(&grants_after);
    let ids_after: Vec<i64> = grants_after["grants"]
        .as_array()
        .expect("grants array")
        .iter()
        .map(|g| g["id"].as_i64().unwrap())
        .collect();
    assert!(!ids_after.contains(&grant_2_id), "grant 2 must be gone: {ids_after:?}");
    assert!(!ids_after.contains(&grant_1_id), "grant 1 (already revoked) must also be gone: {ids_after:?}");
}
