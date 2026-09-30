//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC8 (P2) — Given a user approves consent for an unverified (DCR,
//! non-allowlisted) client, When the grant/consent is recorded, Then the
//! record notes the client was unverified at approval time.

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

#[tokio::test]
async fn approved_unverified_dcr_consent_audit_row_notes_it_was_unverified() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Consent Audit Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    let client_id = register_dcr_client(&http, &server.base_url).await;
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);

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
    assert!(consent_resp.status().is_redirection(), "consent must succeed and redirect: {}", consent_resp.status());

    let audit = tenant_client
        .tools_call("host.oauth.audit", json!({"limit": 50}))
        .await
        .expect("host.oauth.audit");
    let events = common::extract_structured(&audit)["events"].as_array().cloned().unwrap_or_default();
    let consent_event = events
        .iter()
        .find(|e| e["event"] == json!("consent") && e["client_id"] == json!(client_id))
        .expect("the consent event for this unverified client must be in the audit log");
    assert_eq!(
        consent_event["reason"],
        json!("unverified_client_at_approval"),
        "an unverified (DCR, non-allowlisted) client's consent record must note it was unverified at approval: {consent_event:?}"
    );

    let _ = ns;
}
