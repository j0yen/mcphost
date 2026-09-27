//! PRD-mcphost-oauth-demand-signal
//! AC4 (P0) — Given two DCR registrations, one CIMD client, four authorize
//! requests, two consents and two tokens issued in 7 days, When
//! `admin.oauth.demand_stats` runs, Then the funnel reads `{registrations_7d: {dcr:
//! 2, cimd: 1}, authorize_requests_7d: 4, consents_7d: 2, tokens_issued_7d:
//! 2}` and `clients == {cimd: 1, dcr: 2}`.
//!
//! Drives two full authorize -> consent -> token exchanges end to end (one
//! per client: a DCR-registered native app, and a CIMD client identified by
//! its own URL) -- each exchange's `GET` (display) then `POST` (consent)
//! to `/oauth/authorize` is one authorize request apiece, so two exchanges
//! naturally produce exactly four, two consents, and two token issuances. A
//! second DCR client is registered but never used, to prove
//! `registrations_7d.dcr` counts registrations, not usage.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
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

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// GET (display) then POST (consent) to `/oauth/authorize` for `client_id`,
/// proving ownership with `tenant_key` -- returns the minted code. Two
/// authorize-endpoint hits per call, matching this AC's "four authorize
/// requests" over two calls.
#[allow(clippy::too_many_arguments)]
async fn run_authorize_and_consent(
    http: &reqwest::Client,
    base_url: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    tenant_key: &str,
) -> String {
    let resource = format!("{base_url}/mcp");
    let get_resp = http
        .get(format!(
            "{base_url}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}\
             &code_challenge={challenge}&code_challenge_method=S256&state=abc&scope=mcp&resource={}",
            urlencoding_stub(client_id),
            urlencoding_stub(redirect_uri),
            urlencoding_stub(&resource),
        ))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(get_resp.status(), reqwest::StatusCode::OK, "display page must render");

    let consent_resp = http
        .post(format!("{base_url}/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", tenant_key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_base, query) = location.split_once('?').expect("redirect must carry a query string");
    query_param(query, "code").expect("code present in redirect").to_string()
}

fn urlencoding_stub(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

async fn register_dcr_client(http: &reqwest::Client, base_url: &str, redirect_uri: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": [redirect_uri]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

#[allow(clippy::too_many_arguments)]
async fn exchange_code_for_token(
    http: &reqwest::Client,
    base_url: &str,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) {
    let token_resp: Value = http
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    assert!(token_resp["access_token"].as_str().is_some_and(|t| !t.is_empty()), "{token_resp:?}");
}

#[tokio::test]
async fn admin_oauth_stats_funnel_counts_registrations_authorize_consents_and_tokens() {
    let server = TestServer::start().await;
    let (_ns, tenant_key) = signup(&server.base_url, "Funnel Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // Two DCR registrations -- the second is never used in any flow.
    let redirect_uri = "http://127.0.0.1/cb";
    let dcr_client_id = register_dcr_client(&http, &server.base_url, redirect_uri).await;
    let _unused_dcr_client_id = register_dcr_client(&http, &server.base_url, redirect_uri).await;

    // One CIMD client, identified by its own URL.
    let cimd_server = MockServer::start().await;
    let cimd_client_id = format!("{}/cimd.json", cimd_server.uri());
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_client_id,
            "client_name": "Funnel CIMD Connector",
            "redirect_uris": [redirect_uri],
        })))
        .mount(&cimd_server)
        .await;

    // Exchange 1: the DCR client.
    let verifier_1 = "a-pkce-verifier-at-least-43-chars-long-dcr-flow";
    let challenge_1 = code_challenge_for(verifier_1);
    let code_1 = run_authorize_and_consent(&http, &server.base_url, &dcr_client_id, redirect_uri, &challenge_1, &tenant_key)
        .await;
    exchange_code_for_token(&http, &server.base_url, &dcr_client_id, redirect_uri, &code_1, verifier_1).await;

    // Exchange 2: the CIMD client.
    let verifier_2 = "a-pkce-verifier-at-least-43-chars-long-cimd-flow";
    let challenge_2 = code_challenge_for(verifier_2);
    let code_2 =
        run_authorize_and_consent(&http, &server.base_url, &cimd_client_id, redirect_uri, &challenge_2, &tenant_key)
            .await;
    exchange_code_for_token(&http, &server.base_url, &cimd_client_id, redirect_uri, &code_2, verifier_2).await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin.tools_call("admin.oauth.demand_stats", json!({})).await.expect("admin.oauth.demand_stats");
    let body = extract_structured(&result);

    assert_eq!(
        body["funnel"],
        json!({
            "registrations_7d": {"dcr": 2, "cimd": 1},
            "authorize_requests_7d": 4,
            "consents_7d": 2,
            "tokens_issued_7d": 2,
        }),
        "{body:?}"
    );
    assert_eq!(body["clients"], json!({"cimd": 1, "dcr": 2}), "{body:?}");
}
