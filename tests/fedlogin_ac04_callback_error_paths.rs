//! PRD-mcphost-federated-end-user-login
//! AC4 (P0) — Given the callback with a tampered `nonce`, an unknown
//! `state`, a `state` older than 10 min, a provider `error=access_denied`,
//! or an `id_token` signed by another key, When each arrives, Then each
//! renders an inline error with its reason code and no code is minted for
//! the client.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use federation::{query_param, sign_id_token, KID_1};
use serde_json::json;
use sha2::{Digest, Sha256};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

struct Started {
    server: TestServer,
    http: reqwest::Client,
    provider: federation::FakeProvider,
    upstream_state: String,
    upstream_nonce: String,
}

/// Common setup every scenario shares: a tenant with a provider, a
/// registered native client, and one `GET /oauth/authorize` that has
/// already minted a pending row -- each scenario then drives its own
/// (malformed) callback against that same pending row.
async fn start_pending_login() -> Started {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let provider = federation::start().await;
    key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({"issuer": provider.issuer(), "client_id": "acme-client", "client_secret": "acme-secret"}),
        )
        .await
        .expect("provider_set must succeed");

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: serde_json::Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().unwrap().to_string();
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    let authorize_resp = http
        .get(format!("{}/oauth/authorize", server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", code_challenge_for(verifier).as_str()),
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

    Started { server, http, provider, upstream_state, upstream_nonce }
}

/// Asserts the callback response is an inline (non-redirect) error page
/// naming `reason_code`, and that the pending row was never advanced to
/// where a code could be minted (the approval `POST` still refuses).
async fn assert_inline_error_and_no_code(started: &Started, callback_resp: reqwest::Response, reason_code: &str) {
    assert!(!callback_resp.status().is_redirection(), "must not redirect: {}", callback_resp.status());
    let body = callback_resp.text().await.expect("read body");
    assert!(body.contains(reason_code), "body must name '{reason_code}': {body}");

    let approve_resp = started
        .http
        .post(format!("{}/oauth/federation/callback", started.server.base_url))
        .form(&[("token", started.upstream_state.as_str())])
        .send()
        .await
        .expect("POST /oauth/federation/callback");
    assert!(
        !approve_resp.status().is_redirection(),
        "no code may ever be minted for this pending login: {}",
        approve_resp.status()
    );
}

#[tokio::test]
async fn tampered_nonce_is_refused() {
    let started = start_pending_login().await;
    let id_token = sign_id_token(
        KID_1,
        federation::priv_pem_1(),
        &started.provider.issuer(),
        "acme-client",
        "u1",
        "a-completely-different-nonce",
        Some("u1@acme.test"),
        Some(true),
        Some("U One"),
        300,
    );
    federation::mount_token(&started.provider, &id_token).await;
    let resp = started
        .http
        .get(format!("{}/oauth/federation/callback", started.server.base_url))
        .query(&[("code", "upstream-code"), ("state", started.upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    assert_inline_error_and_no_code(&started, resp, "nonce_mismatch").await;
}

#[tokio::test]
async fn unknown_state_is_refused() {
    let started = start_pending_login().await;
    let resp = started
        .http
        .get(format!("{}/oauth/federation/callback", started.server.base_url))
        .query(&[("code", "upstream-code"), ("state", "a-state-nobody-ever-issued")])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    assert_inline_error_and_no_code(&started, resp, "unknown_state").await;
}

#[tokio::test]
async fn state_older_than_ten_minutes_is_refused() {
    let started = start_pending_login().await;
    let past = mcphost::state::now_unix() - 11 * 60;
    started
        .server
        .state
        .db
        .test_backdate_oauth_federation_pending(started.upstream_state.clone(), past)
        .await
        .expect("backdate pending row");

    let id_token = sign_id_token(
        KID_1,
        federation::priv_pem_1(),
        &started.provider.issuer(),
        "acme-client",
        "u1",
        &started.upstream_nonce,
        Some("u1@acme.test"),
        Some(true),
        Some("U One"),
        300,
    );
    federation::mount_token(&started.provider, &id_token).await;
    let resp = started
        .http
        .get(format!("{}/oauth/federation/callback", started.server.base_url))
        .query(&[("code", "upstream-code"), ("state", started.upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    assert_inline_error_and_no_code(&started, resp, "state_expired").await;
}

#[tokio::test]
async fn provider_error_access_denied_is_refused() {
    let started = start_pending_login().await;
    let resp = started
        .http
        .get(format!("{}/oauth/federation/callback", started.server.base_url))
        .query(&[("error", "access_denied"), ("state", started.upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    assert_inline_error_and_no_code(&started, resp, "access_denied").await;
}

#[tokio::test]
async fn id_token_signed_by_another_key_is_refused() {
    let started = start_pending_login().await;
    // Signed with `priv_pem_2` (never published in the provider's own
    // JWKS) while the JWT header still names `KID_1` -- the published
    // key's signature check fails.
    let id_token = sign_id_token(
        KID_1,
        federation::priv_pem_2(),
        &started.provider.issuer(),
        "acme-client",
        "u1",
        &started.upstream_nonce,
        Some("u1@acme.test"),
        Some(true),
        Some("U One"),
        300,
    );
    federation::mount_token(&started.provider, &id_token).await;
    let resp = started
        .http
        .get(format!("{}/oauth/federation/callback", started.server.base_url))
        .query(&[("code", "upstream-code"), ("state", started.upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    assert_inline_error_and_no_code(&started, resp, "invalid_id_token").await;
}
