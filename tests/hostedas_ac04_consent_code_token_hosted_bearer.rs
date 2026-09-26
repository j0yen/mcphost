//! PRD-mcphost-hosted-authorization-server
//! AC4 (P0) — Given tenant T's key, When the consent POST proves it with a
//! valid `code_challenge` (S256), `state=abc`, `resource=<public>/mcp`,
//! Then the redirect carries `code` and `state=abc`; When `POST
//! /oauth/token` presents the matching `code_verifier`, Then a JWT with
//! `iss=<public>`, `sub=<T public id>`, `aud=<public>/mcp`, `scope=mcp`
//! and a refresh token return; When that JWT calls `host.state.get` on
//! `/mcp`, Then it runs as T with `auth_method == hosted_token`.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use serde_json::{Value, json};

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

#[tokio::test]
async fn consent_mints_code_token_exchange_mints_hosted_jwt_that_runs_as_tenant() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Hosted AS Tenant").await;

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // Register a native DCR client (a public client, no secret) to authorize.
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/mcp", server.base_url);

    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp
        .headers()
        .get("location")
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string();
    let (redirect_base, query) = location.split_once('?').expect("redirect must carry a query string");
    assert_eq!(redirect_base, "http://127.0.0.1/cb");
    assert_eq!(query_param(query, "state"), Some("abc"), "state must be echoed verbatim: {location}");
    let code = query_param(query, "code").expect("code present in redirect").to_string();
    assert!(!code.is_empty());

    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");

    assert_eq!(token_resp["token_type"], json!("Bearer"));
    assert_eq!(token_resp["expires_in"], json!(3600));
    assert_eq!(token_resp["scope"], json!("mcp"));
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();
    assert!(token_resp["refresh_token"].as_str().is_some_and(|r| !r.is_empty()));

    // JWT claims, read without signature verification (a test convenience;
    // `oauth_ac03`-style suites already pin signature-rejection behavior) --
    // just confirms iss/sub/aud/scope shape the token endpoint promised.
    let payload_b64 = access_token.split('.').nth(1).expect("JWT has a payload segment");
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).expect("base64url decode payload");
    let claims: Value = serde_json::from_slice(&payload_bytes).expect("parse JWT claims");
    assert_eq!(claims["iss"], json!(server.base_url));
    assert_eq!(claims["sub"], json!(ns));
    assert_eq!(claims["aud"], json!(resource));
    assert_eq!(claims["scope"], json!("mcp"));

    let bearer_client = McpClient::with_bearer(&server.base_url, &access_token);
    bearer_client
        .tools_call("host.state.set", json!({"key": "hosted", "value": "yes"}))
        .await
        .expect("host.state.set via hosted token must succeed");
    let get_result = bearer_client
        .tools_call("host.state.get", json!({"key": "hosted"}))
        .await
        .expect("host.state.get via hosted token must succeed");
    let get_structured = common::extract_structured(&get_result);
    assert_eq!(get_structured["found"], json!(true));
    assert_eq!(get_structured["value"], json!("yes"));

    let whoami_result = bearer_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami via hosted token must succeed");
    let whoami = common::extract_structured(&whoami_result);
    assert_eq!(whoami["tenant"], json!(ns), "must run as the consenting tenant");
    assert_eq!(whoami["auth_method"], json!("hosted_token"));
}
