//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC9 (P0) — Given an existing CIMD or first-party (allowlisted) consent
//! flow, When a user completes it, Then behavior is unchanged — consent
//! still cannot be skipped and scopes still render — and the caution is
//! purely additive to the DCR non-allowlisted case.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REDIRECT_URI: &str = "https://client.example/callback";

async fn cimd_server_with_doc() -> (MockServer, String) {
    let server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", server.uri());
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_url,
            "client_name": "Verified Connector",
            "redirect_uris": [REDIRECT_URI],
        })))
        .mount(&server)
        .await;
    (server, cimd_url)
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

#[tokio::test]
async fn cimd_consent_flow_renders_scopes_cannot_be_skipped_and_carries_no_caution() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Unchanged Flow Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    let (_cimd, cimd_url) = cimd_server_with_doc().await;

    key_client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("host.oauth.scope_set read must succeed");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["read"]}),
        )
        .await
        .expect("publish search must succeed");

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, &ns);
    let authorize_form = [
        ("response_type", "code"),
        ("client_id", cimd_url.as_str()),
        ("redirect_uri", REDIRECT_URI),
        ("code_challenge", "dummy-challenge"),
        ("code_challenge_method", "S256"),
        ("state", "abc"),
        ("scope", "read"),
        ("resource", resource.as_str()),
    ];

    // GET: scopes still render, and this verified (CIMD) client carries no
    // unverified caution -- additive-only, not a behavior change.
    let mut get_url = reqwest::Url::parse(&format!("{}/oauth/authorize", server.base_url)).unwrap();
    {
        let mut pairs = get_url.query_pairs_mut();
        for (k, v) in &authorize_form {
            pairs.append_pair(k, v);
        }
    }
    let get_resp = http.get(get_url).send().await.expect("GET /oauth/authorize");
    assert!(get_resp.status().is_success(), "consent page must render: {}", get_resp.status());
    let page = get_resp.text().await.expect("consent page body");
    assert!(page.contains("data-scope=\"read\""), "scopes must still render: {page}");
    assert!(page.contains("tenant_key"), "the key/claim form must still be present -- consent cannot be skipped: {page}");
    assert!(
        !page.to_lowercase().contains("unverified"),
        "a CIMD client's consent page must carry no unverified caution: {page}"
    );

    // POST without proof of ownership: re-renders consent (never skips,
    // never mints a code) -- unchanged pre-existing behavior.
    let unproven = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&authorize_form)
        .send()
        .await
        .expect("POST /oauth/authorize without proof");
    assert!(
        !unproven.status().is_redirection(),
        "consent must not be skippable without proof of tenant ownership: {}",
        unproven.status()
    );
    let unproven_body = unproven.text().await.expect("body");
    assert!(unproven_body.contains("tenant_key"), "must re-render the consent form, not skip it: {unproven_body}");

    // POST with proof: mints a code and redirects, exactly as before.
    let mut form = authorize_form.to_vec();
    form.push(("tenant_key", key.as_str()));
    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&form)
        .send()
        .await
        .expect("POST /oauth/authorize with proof");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect must carry a query string");
    assert!(query_param(query, "code").is_some(), "a code must be minted: {location}");
    assert_eq!(query_param(query, "state"), Some("abc"));
}
