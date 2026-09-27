//! PRD-mcphost-oauth-client-policy
//! AC1 (P0) — Given tenant T with `clients: allowlist` naming client A's
//! id and CIMD host `cimd.test`, When A, a CIMD client from `cimd.test`,
//! and a DCR client B start `/oauth/authorize`, Then A and the CIMD client
//! reach consent and B gets `client_not_allowed` rendered inline with no
//! redirect and an audit row.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// Mounts a CIMD document at a loopback wiremock server -- its `client_id`
/// URL's host (`127.0.0.1`, a literal IP, so no real DNS resolution is
/// needed) stands in for the PRD's illustrative `cimd.test`: what this AC
/// actually proves is that a policy's `cimd_host` entry matches a CIMD
/// client's document host, not that the literal string "cimd.test" is
/// special.
async fn cimd_server(redirect_uri: &str) -> (MockServer, String, String) {
    let server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", server.uri());
    let host = reqwest::Url::parse(&cimd_url).unwrap().host_str().unwrap().to_string();
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_url,
            "client_name": "CIMD Connector",
            "redirect_uris": [redirect_uri],
        })))
        .mount(&server)
        .await;
    (server, cimd_url, host)
}

#[allow(clippy::too_many_arguments)]
async fn authorize_post(
    http: &reqwest::Client,
    base_url: &str,
    client_id: &str,
    redirect_uri: &str,
    resource: &str,
    tenant_key: &str,
    verifier: &str,
    state_param: &str,
) -> reqwest::Response {
    http.post(format!("{base_url}/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_challenge", code_challenge_for(verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", state_param),
            ("scope", "mcp"),
            ("resource", resource),
            ("tenant_key", tenant_key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize")
}

#[tokio::test]
async fn allowlist_admits_named_client_and_cimd_host_refuses_unlisted_dcr_client() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Allowlist Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let client_a = register_dcr_client(&http, &server.base_url).await;
    let client_b = register_dcr_client(&http, &server.base_url).await;
    let (_cimd_mock, cimd_client_id, cimd_host) = cimd_server("https://cimd-client.example/callback").await;

    let policy = tenant_client
        .tools_call(
            "host.oauth.policy_set",
            json!({"clients": "allowlist", "allowlist": [client_a, cimd_host]}),
        )
        .await
        .expect("policy_set");
    assert_eq!(common::extract_structured(&policy)["clients"], json!("allowlist"));

    let resource = format!("{}/mcp", server.base_url);

    // Client A (allowlisted by client_id) reaches consent: a redirect
    // carrying a fresh code.
    let resp_a = authorize_post(
        &http,
        &server.base_url,
        &client_a,
        "http://127.0.0.1/cb",
        &resource,
        &key,
        "verifier-for-client-a-at-least-43-characters-long",
        "state-a",
    )
    .await;
    assert!(resp_a.status().is_redirection(), "client A must reach consent: {}", resp_a.status());
    let location_a = resp_a.headers().get("location").expect("Location header for A").to_str().unwrap().to_string();
    assert!(location_a.contains("code="), "A's redirect must carry a code: {location_a}");

    // The CIMD client (allowlisted by its document's host) also reaches
    // consent.
    let resp_cimd = authorize_post(
        &http,
        &server.base_url,
        &cimd_client_id,
        "https://cimd-client.example/callback",
        &resource,
        &key,
        "verifier-for-cimd-client-at-least-43-characters-long",
        "state-cimd",
    )
    .await;
    assert!(resp_cimd.status().is_redirection(), "the CIMD client must reach consent: {}", resp_cimd.status());
    let location_cimd =
        resp_cimd.headers().get("location").expect("Location header for CIMD client").to_str().unwrap().to_string();
    assert!(location_cimd.contains("code="), "CIMD client's redirect must carry a code: {location_cimd}");

    // Client B (a DCR client, not on the allowlist) is refused inline,
    // with no redirect at all.
    let resp_b = authorize_post(
        &http,
        &server.base_url,
        &client_b,
        "http://127.0.0.1/cb",
        &resource,
        &key,
        "verifier-for-client-b-at-least-43-characters-long",
        "state-b",
    )
    .await;
    assert!(
        resp_b.headers().get("location").is_none(),
        "client B must not be redirected anywhere: {:?}",
        resp_b.headers().get("location")
    );
    let body_b = resp_b.text().await.expect("body");
    assert!(body_b.contains("client_not_allowed"), "client B's body must name client_not_allowed: {body_b}");

    // An audit row records B's refusal, readable by the tenant.
    let audit = tenant_client
        .tools_call("host.oauth.audit", json!({}))
        .await
        .expect("host.oauth.audit");
    let events = common::extract_structured(&audit)["events"].as_array().cloned().unwrap_or_default();
    assert!(
        events.iter().any(|e| e["client_id"] == json!(client_b) && e["reason"] == json!("client_not_allowed")),
        "audit must contain client B's refusal: {events:?}"
    );
}
