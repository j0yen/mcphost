//! PRD-mcphost-federated-end-user-login
//! AC10 (P0) — Given the hosted AS PRD's test suite and the key-based
//! suites, When run, Then they pass unchanged.
//!
//! The real proof is the full `cargo test` suite (every pre-existing
//! `hostedas_ac*.rs`/`oauthrs_ac*.rs`/`ac*.rs` file, unmodified, still
//! green with this PRD's code landed). This fedlogin-prefixed file is
//! this AC's own proof point per the "every non-deferred AC has its own
//! test file" convention, mirroring `hostedas_ac10`'s own shape: it drives
//! the key-based path, the issuer-JWT path, AND the hosted AS's own
//! root-resource consent/token/hosted-bearer path end to end on a tenant
//! that ALSO has a federated provider registered on its per-tenant
//! resource -- proving the provider's presence disturbs none of them
//! (the root resource keeps the pre-existing "any tenant's key" consent;
//! only the per-tenant resource redirects upstream).

use crate::common;
use crate::federation;
use crate::oauth;

use common::{ADMIN_KEY, McpClient, TestServer, publish, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oauth::{KID_1 as ISSUER_KID_1, jwk_1 as issuer_jwk_1, jwks_server, priv_pem_1 as issuer_priv_pem_1, sign};
use serde_json::json;
use sha2::{Digest, Sha256};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

#[tokio::test]
async fn hosted_as_key_and_issuer_jwt_flows_still_work_alongside_a_federated_provider() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Unchanged Flows Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    // This tenant ALSO registers a federated provider -- its per-tenant
    // resource redirects upstream now, but the root resource and every
    // other pre-existing path must not be disturbed by its presence.
    let provider = federation::start().await;
    key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({"issuer": provider.issuer(), "client_id": "acme-client", "client_secret": "acme-secret"}),
        )
        .await
        .expect("provider_set must succeed");

    // Key-based path: signup -> publish -> call -> admin, unchanged.
    let qualified = publish(
        &key_client,
        "hello",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.hello"));
    let result = key_client
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("calling a key-authenticated tenant's own published tool must still work");
    assert_eq!(common::extract_structured(&result)["msg"], json!("hi"));

    let key_whoami = common::extract_structured(&key_client.tools_call("host.whoami", json!({})).await.expect("host.whoami via key"));
    assert_eq!(key_whoami["tenant"], json!(ns));
    assert_eq!(key_whoami["auth_method"], json!("key"), "whoami: {key_whoami}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listed = common::extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    assert!(
        listed["tenants"].as_array().expect("tenants array").iter().any(|t| t["tenant"] == json!(ns)),
        "admin.tenants must still list the key-based tenant: {listed}"
    );

    // Issuer-JWT path (bring-your-own-issuer bearer auth, PRD-mcphost-oauth-resource-server):
    // unrelated to this PRD's own OIDC-login provider, must still run as
    // this tenant.
    let jwks = jwks_server(issuer_jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    let issuer_token = sign(ISSUER_KID_1, issuer_priv_pem_1(), issuer, audience, "user-42", 300);
    let issuer_bearer_client = McpClient::with_bearer(&server.base_url, &issuer_token);
    let issuer_whoami =
        common::extract_structured(&issuer_bearer_client.tools_call("host.whoami", json!({})).await.expect("host.whoami via issuer JWT"));
    assert_eq!(issuer_whoami["tenant"], json!(ns), "must run as the issuer's registered tenant");
    assert_eq!(issuer_whoami["subject"], json!("user-42"));
    assert_eq!(issuer_whoami["auth_method"], json!("oauth"), "whoami: {issuer_whoami}");

    // Hosted AS's own root-resource consent/token/hosted-bearer path
    // (PRD-mcphost-hosted-authorization-server): a DCR client authorizing
    // on the ROOT resource still gets the tenant-owner key/claim consent
    // form -- the provider registered above only changes the PER-TENANT
    // resource's behavior.
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
    let client_id = register["client_id"].as_str().expect("client_id").to_string();

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let root_resource = format!("{}/mcp", server.base_url);
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
            ("resource", root_resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "root-resource consent must still mint a code directly: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let code = query.split('&').find_map(|p| p.strip_prefix("code=")).expect("code present").to_string();

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
    let hosted_access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();
    let hosted_bearer_client = McpClient::with_bearer(&server.base_url, &hosted_access_token);
    let hosted_whoami =
        common::extract_structured(&hosted_bearer_client.tools_call("host.whoami", json!({})).await.expect("host.whoami via hosted token"));
    assert_eq!(hosted_whoami["tenant"], json!(ns));
    assert_eq!(hosted_whoami["auth_method"], json!("hosted_token"), "whoami: {hosted_whoami}");
}
