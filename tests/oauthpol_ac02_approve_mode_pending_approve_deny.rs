//! PRD-mcphost-oauth-client-policy
//! AC2 (P0) — Given `clients: approve`, When new client C authorizes,
//! Then the page reads "awaiting approval", `host.oauth.pending` lists C,
//! and after `host.oauth.client_approve` C's next authorize reaches
//! consent; `host.oauth.client_deny` removes it and later attempts read
//! `client_not_allowed`.

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

#[allow(clippy::too_many_arguments)]
async fn authorize_post(
    http: &reqwest::Client,
    base_url: &str,
    client_id: &str,
    resource: &str,
    tenant_key: &str,
    verifier: &str,
    state_param: &str,
) -> reqwest::Response {
    http.post(format!("{base_url}/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", "http://127.0.0.1/cb"),
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
async fn approve_mode_queues_then_approve_reaches_consent_and_deny_stays_refused() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Approve Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    tenant_client
        .tools_call("host.oauth.policy_set", json!({"clients": "approve"}))
        .await
        .expect("policy_set approve");

    let client_c = register_dcr_client(&http, &server.base_url).await;
    let client_d = register_dcr_client(&http, &server.base_url).await;

    // First authorize: C is new, so it's held pending -- no redirect, the
    // page reads "awaiting approval".
    let first = authorize_post(
        &http,
        &server.base_url,
        &client_c,
        &resource,
        &key,
        "verifier-for-client-c-first-attempt-43-chars-min",
        "state-1",
    )
    .await;
    assert!(first.headers().get("location").is_none(), "a pending client must not be redirected");
    let body = first.text().await.expect("body");
    assert!(
        body.to_lowercase().contains("awaiting approval"),
        "pending page must read 'awaiting approval': {body}"
    );

    let pending = tenant_client.tools_call("host.oauth.pending", json!({})).await.expect("host.oauth.pending");
    let pending_ids: Vec<String> = common::extract_structured(&pending)["pending"]
        .as_array()
        .expect("pending array")
        .iter()
        .map(|p| p["client_id"].as_str().unwrap().to_string())
        .collect();
    assert!(pending_ids.contains(&client_c), "host.oauth.pending must list C: {pending_ids:?}");

    tenant_client
        .tools_call("host.oauth.client_approve", json!({"client_id": client_c}))
        .await
        .expect("client_approve");

    // C's next authorize now reaches consent (a redirect carrying a code).
    let second = authorize_post(
        &http,
        &server.base_url,
        &client_c,
        &resource,
        &key,
        "verifier-for-client-c-second-attempt-43-chars-min",
        "state-2",
    )
    .await;
    assert!(second.status().is_redirection(), "approved client C must reach consent: {}", second.status());
    let location = second.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    assert!(location.contains("code="), "C's redirect must carry a code: {location}");

    // D goes pending too, then gets denied.
    let d_first = authorize_post(
        &http,
        &server.base_url,
        &client_d,
        &resource,
        &key,
        "verifier-for-client-d-first-attempt-43-chars-min",
        "state-3",
    )
    .await;
    assert!(d_first.headers().get("location").is_none());
    let d_pending = tenant_client.tools_call("host.oauth.pending", json!({})).await.expect("host.oauth.pending");
    let d_pending_ids: Vec<String> = common::extract_structured(&d_pending)["pending"]
        .as_array()
        .expect("pending array")
        .iter()
        .map(|p| p["client_id"].as_str().unwrap().to_string())
        .collect();
    assert!(d_pending_ids.contains(&client_d), "host.oauth.pending must list D: {d_pending_ids:?}");

    tenant_client
        .tools_call("host.oauth.client_deny", json!({"client_id": client_d}))
        .await
        .expect("client_deny");

    let d_after_deny = tenant_client.tools_call("host.oauth.pending", json!({})).await.expect("host.oauth.pending");
    let d_after_deny_ids: Vec<String> = common::extract_structured(&d_after_deny)["pending"]
        .as_array()
        .expect("pending array")
        .iter()
        .map(|p| p["client_id"].as_str().unwrap().to_string())
        .collect();
    assert!(!d_after_deny_ids.contains(&client_d), "D must be removed from pending after deny");

    // D's later attempts read client_not_allowed, never pending again.
    let d_second = authorize_post(
        &http,
        &server.base_url,
        &client_d,
        &resource,
        &key,
        "verifier-for-client-d-second-attempt-43-chars-min",
        "state-4",
    )
    .await;
    assert!(d_second.headers().get("location").is_none(), "denied client D must not be redirected");
    let d_second_body = d_second.text().await.expect("body");
    assert!(
        d_second_body.contains("client_not_allowed"),
        "denied client D must read client_not_allowed: {d_second_body}"
    );
}
