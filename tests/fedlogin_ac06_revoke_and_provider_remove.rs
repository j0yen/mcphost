//! PRD-mcphost-federated-end-user-login
//! AC6 (P0) — Given u1's grant, When acme calls `host.enduser.revoke
//! {subject}`, Then u1's access token fails within 60s and the refresh
//! token returns `invalid_grant`; When acme calls
//! `host.oauth.provider_remove`, Then every federated grant is revoked
//! and the per-tenant resource falls back to owner login.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, extract_structured, signup};
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
    key_client: McpClient,
    provider: federation::FakeProvider,
    resource: String,
    client_id: String,
}

/// Runs a full federated login for `sub` and returns `(access_token,
/// refresh_token)` -- shared by both this AC's scenarios (u1's own revoke,
/// then a second user for the provider_remove scenario).
async fn federated_login(ctx: &Ctx, sub: &str) -> (String, String) {
    let verifier = format!("verifier-for-{sub}-at-least-43-characters-long!!");
    let authorize_resp = ctx
        .http
        .get(format!("{}/oauth/authorize", ctx.server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", ctx.client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", code_challenge_for(&verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", ctx.resource.as_str()),
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
        &ctx.provider.issuer(),
        "acme-client",
        sub,
        &upstream_nonce,
        Some(&format!("{sub}@acme.test")),
        Some(true),
        Some("Some User"),
        300,
    );
    federation::mount_token(&ctx.provider, &id_token).await;
    ctx.http
        .get(format!("{}/oauth/federation/callback", ctx.server.base_url))
        .query(&[("code", "upstream-code"), ("state", upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    let approve_resp = ctx
        .http
        .post(format!("{}/oauth/federation/callback", ctx.server.base_url))
        .form(&[("token", upstream_state.as_str())])
        .send()
        .await
        .expect("POST /oauth/federation/callback");
    let approve_location = approve_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, approve_query) = approve_location.split_once('?').unwrap();
    let code = query_param(approve_query, "code").unwrap().to_string();

    let token_resp: Value = ctx
        .http
        .post(format!("{}/oauth/token", ctx.server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", ctx.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let access_token = token_resp["access_token"].as_str().expect("access_token").to_string();
    let refresh_token = token_resp["refresh_token"].as_str().expect("refresh_token").to_string();
    (access_token, refresh_token)
}

async fn setup() -> Ctx {
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
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    Ctx { server, http, key_client, provider, resource, client_id }
}

#[tokio::test]
async fn enduser_revoke_fails_access_and_refresh_token() {
    let ctx = setup().await;
    let (access_token, refresh_token) = federated_login(&ctx, "u1").await;
    let subject = format!("{}#u1", ctx.provider.issuer());

    let revoke_result = extract_structured(
        &ctx.key_client
            .tools_call("host.enduser.revoke", json!({"subject": subject}))
            .await
            .expect("revoke must succeed"),
    );
    assert_eq!(revoke_result["revoked"], json!(true), "{revoke_result}");

    let bearer_client = McpClient::with_bearer(&ctx.server.base_url, &access_token);
    let err = bearer_client
        .tools_call("host.state.get", json!({"key": "anything"}))
        .await
        .expect_err("a revoked federated access token must fail");
    assert_eq!(err.error_code.as_deref(), Some("invalid_token"), "{err:?}");

    let refresh_resp: Value = ctx
        .http
        .post(format!("{}/oauth/token", ctx.server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    assert_eq!(refresh_resp["error"], json!("invalid_grant"), "{refresh_resp}");
}

#[tokio::test]
async fn provider_remove_revokes_every_federated_grant_and_falls_back_to_owner_login() {
    let ctx = setup().await;
    let (access_token, _refresh_token) = federated_login(&ctx, "u2").await;

    let removed = extract_structured(
        &ctx.key_client.tools_call("host.oauth.provider_remove", json!({})).await.expect("provider_remove must succeed"),
    );
    assert_eq!(removed["removed"], json!(true), "{removed}");

    let bearer_client = McpClient::with_bearer(&ctx.server.base_url, &access_token);
    let err = bearer_client
        .tools_call("host.state.get", json!({"key": "anything"}))
        .await
        .expect_err("every federated grant must be revoked when the provider is removed");
    assert_eq!(err.error_code.as_deref(), Some("invalid_token"), "{err:?}");

    // The per-tenant resource now falls back to owner (key/claim) login:
    // `GET /oauth/authorize` renders the consent form instead of
    // redirecting upstream (no provider to redirect to any more).
    let authorize_resp = ctx
        .http
        .get(format!("{}/oauth/authorize", ctx.server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", ctx.client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", "any-challenge-value-at-least-43-characters-long"),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
            ("scope", "mcp"),
            ("resource", ctx.resource.as_str()),
        ])
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(authorize_resp.status(), reqwest::StatusCode::OK, "must render the owner-login consent page, not redirect");
    let body = authorize_resp.text().await.expect("read body");
    assert!(body.contains("tenant_key"), "the key/claim form must be present: {body}");
}
