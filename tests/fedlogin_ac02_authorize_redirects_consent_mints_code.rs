//! PRD-mcphost-federated-end-user-login
//! AC2 (P0) — Given a registered native client, When it starts
//! `/oauth/authorize` with `resource=<public>/t/acme/mcp`, Then the
//! response redirects to the provider's authorization endpoint with
//! `code_challenge_method=S256`, a `nonce` and a `state`; When the
//! provider redirects back to `/oauth/federation/callback` with a valid
//! code, Then mcphost's consent page renders and, on approval, the client
//! receives its code and `state`.

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

#[tokio::test]
async fn authorize_redirects_upstream_then_consent_mints_code_for_original_client() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let provider = federation::start().await;
    key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({
                "issuer": provider.issuer(),
                "client_id": "acme-client",
                "client_secret": "acme-secret",
            }),
        )
        .await
        .expect("provider_set must succeed");

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // A registered native DCR client -- the ORIGINAL client asking for
    // access to acme's per-tenant resource.
    let register: serde_json::Value = http
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
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    let authorize_resp = http
        .get(format!("{}/oauth/authorize", server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "client-state-abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert!(authorize_resp.status().is_redirection(), "must redirect upstream: {}", authorize_resp.status());
    let location = authorize_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (redirect_base, query) = location.split_once('?').expect("redirect must carry a query string");
    assert_eq!(redirect_base, provider.authorization_endpoint());
    assert_eq!(query_param(query, "client_id"), Some("acme-client"));
    assert_eq!(query_param(query, "response_type"), Some("code"));
    assert_eq!(query_param(query, "code_challenge_method"), Some("S256"));
    let upstream_nonce = query_param(query, "nonce").expect("nonce present").to_string();
    let upstream_state = query_param(query, "state").expect("state present").to_string();
    assert!(!upstream_nonce.is_empty());
    assert!(!upstream_state.is_empty());
    assert_ne!(upstream_state, "client-state-abc", "the upstream state must be mcphost's own opaque token, not the client's");
    assert!(query_param(query, "code_challenge").is_some_and(|c| !c.is_empty()));

    // The provider "redirects back" -- simulated directly, per this suite's
    // fixture doc comment.
    let id_token = sign_id_token(
        KID_1,
        federation::priv_pem_1(),
        &provider.issuer(),
        "acme-client",
        "u1",
        &upstream_nonce,
        Some("u1@acme.test"),
        Some(true),
        Some("U One"),
        300,
    );
    federation::mount_token(&provider, &id_token).await;

    let callback_resp = http
        .get(format!("{}/oauth/federation/callback", server.base_url))
        .query(&[("code", "upstream-code-xyz"), ("state", upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    assert_eq!(callback_resp.status(), reqwest::StatusCode::OK, "must render the consent page");
    let body = callback_resp.text().await.expect("read consent page body");
    assert!(body.contains("Authorize"), "consent page must render: {body}");
    assert!(body.contains(&upstream_state), "hidden token field must carry the pending row's own state: {body}");

    let approve_resp = http
        .post(format!("{}/oauth/federation/callback", server.base_url))
        .form(&[("token", upstream_state.as_str())])
        .send()
        .await
        .expect("POST /oauth/federation/callback");
    assert!(approve_resp.status().is_redirection(), "approval must redirect to the original client: {}", approve_resp.status());
    let approve_location = approve_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (approve_base, approve_query) = approve_location.split_once('?').expect("redirect must carry a query string");
    assert_eq!(approve_base, "http://127.0.0.1/cb");
    assert_eq!(query_param(approve_query, "state"), Some("client-state-abc"), "the client's own state must be echoed back");
    let code = query_param(approve_query, "code").expect("code present").to_string();
    assert!(!code.is_empty());

    // The client redeems its code exactly like any other authorization-code
    // exchange (hostedas_ac04's own shape).
    let token_resp: serde_json::Value = http
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
    assert!(token_resp["access_token"].as_str().is_some_and(|t| !t.is_empty()), "{token_resp}");
}
