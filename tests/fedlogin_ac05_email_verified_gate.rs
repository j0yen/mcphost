//! PRD-mcphost-federated-end-user-login
//! AC5 (P0) — Given the provider returns `email_verified: false`, When the
//! tenant keeps the default, Then the login is refused with
//! `email_unverified`; When the tenant set `require_verified_email:
//! false`, Then it succeeds and `MCPHOST_END_USER_EMAIL` is still set.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use federation::{query_param, sign_id_token, KID_1};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

struct Ctx {
    server: TestServer,
    http: reqwest::Client,
    client_id: String,
    verifier: String,
    upstream_state: String,
}

/// Runs the upstream leg through to `GET /oauth/federation/callback` with
/// an `id_token` carrying `email_verified: false`, and returns that
/// response alongside everything a scenario needs to continue the flow --
/// `require_verified_email` toggles what `host.oauth.provider_set` was
/// called with.
async fn callback_with_unverified_email(require_verified_email: Option<bool>) -> (Ctx, reqwest::Response) {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let provider = federation::start().await;
    let mut args = json!({"issuer": provider.issuer(), "client_id": "acme-client", "client_secret": "acme-secret"});
    if let Some(rve) = require_verified_email {
        args["require_verified_email"] = Value::Bool(rve);
    }
    key_client.tools_call("host.oauth.provider_set", args).await.expect("provider_set must succeed");

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().unwrap().to_string();
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism".to_string();
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    let authorize_resp = http
        .get(format!("{}/oauth/authorize", server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", code_challenge_for(&verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("GET /oauth/authorize");
    let location = authorize_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let upstream_nonce = query_param(query, "nonce").unwrap().to_string();
    let upstream_state = query_param(query, "state").unwrap().to_string();

    let id_token = sign_id_token(
        KID_1,
        federation::priv_pem_1(),
        &provider.issuer(),
        "acme-client",
        "u1",
        &upstream_nonce,
        Some("u1@acme.test"),
        Some(false),
        Some("U One"),
        300,
    );
    federation::mount_token(&provider, &id_token).await;
    let callback_resp = http
        .get(format!("{}/oauth/federation/callback", server.base_url))
        .query(&[("code", "upstream-code"), ("state", upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    (Ctx { server, http, client_id, verifier, upstream_state }, callback_resp)
}

#[tokio::test]
async fn default_refuses_unverified_email() {
    let (_ctx, resp) = callback_with_unverified_email(None).await;
    assert!(!resp.status().is_redirection(), "must not redirect: {}", resp.status());
    let body = resp.text().await.expect("read body");
    assert!(body.contains("email_unverified"), "body must name email_unverified: {body}");
}

#[tokio::test]
async fn require_verified_email_false_succeeds_with_email_still_set() {
    let (ctx, resp) = callback_with_unverified_email(Some(false)).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "consent page must render");
    let body = resp.text().await.expect("read body");
    assert!(body.contains("Authorize"), "consent page must render: {body}");

    let approve_resp = ctx
        .http
        .post(format!("{}/oauth/federation/callback", ctx.server.base_url))
        .form(&[("token", ctx.upstream_state.as_str())])
        .send()
        .await
        .expect("POST /oauth/federation/callback");
    assert!(approve_resp.status().is_redirection(), "approval must redirect: {}", approve_resp.status());
    let location = approve_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").expect("code present").to_string();

    let token_resp: Value = ctx
        .http
        .post(format!("{}/oauth/token", ctx.server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", ctx.client_id.as_str()),
            ("code_verifier", ctx.verifier.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();

    let payload_b64 = access_token.split('.').nth(1).expect("JWT has a payload segment");
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).expect("base64url decode payload");
    let claims: Value = serde_json::from_slice(&payload_bytes).expect("parse JWT claims");
    assert_eq!(claims["email"], json!("u1@acme.test"), "MCPHOST_END_USER_EMAIL's own claim source must still be set: {claims}");
}
