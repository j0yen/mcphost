//! PRD-mcphost-oauth-client-policy
//! AC7 (P0) — Given `admin.oauth.client_block {cimd_host: "evil.test"}`,
//! When any tenant's client from that host registers, authorizes or
//! exchanges, Then each is refused `client_blocked` and counted in
//! `admin.oauth.stats`; a tenant key calling `admin.oauth.client_block` is
//! rejected like every other `admin.*` tool.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
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

async fn cimd_server(redirect_uri: &str) -> (MockServer, String, String) {
    let server = MockServer::start().await;
    let cimd_url = format!("{}/cimd.json", server.uri());
    let host = reqwest::Url::parse(&cimd_url).unwrap().host_str().unwrap().to_string();
    Mock::given(method("GET"))
        .and(path("/cimd.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "client_id": cimd_url,
            "client_name": "Blockable Connector",
            "redirect_uris": [redirect_uri],
        })))
        .mount(&server)
        .await;
    (server, cimd_url, host)
}

#[tokio::test]
async fn blocked_cimd_host_is_refused_at_authorize_and_exchange_and_counted() {
    let server = TestServer::start().await;
    let admin_client = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (_ns_a, key_a) = signup(&server.base_url, "AC7 Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "AC7 Tenant B").await;
    let tenant_a = McpClient::with_bearer(&server.base_url, &key_a);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    // A tenant key must never reach admin.oauth.client_block, same as any
    // other admin.* tool.
    let rejected = tenant_a
        .tools_call("admin.oauth.client_block", json!({"cimd_host": "evil.test"}))
        .await
        .expect_err("a tenant key must never reach admin.oauth.client_block");
    assert_eq!(rejected.error_code.as_deref(), Some("forbidden"));

    let (_cimd_mock, cimd_client_id, cimd_host) = cimd_server("https://blockable.example/callback").await;

    // Before the block: tenant A completes a real authorize, minting a
    // code (never exchanged yet) -- proves the block, once applied, also
    // refuses an in-flight exchange, not just a fresh authorize.
    let verifier = "verifier-for-blockable-client-at-least-43-chars";
    let pre_block_consent = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", cimd_client_id.as_str()),
            ("redirect_uri", "https://blockable.example/callback"),
            ("code_challenge", code_challenge_for(verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", "pre-block"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key_a.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize before block");
    assert!(pre_block_consent.status().is_redirection(), "must reach consent before the block exists");
    let location = pre_block_consent.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_base, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").expect("code present").to_string();

    let block = admin_client
        .tools_call("admin.oauth.client_block", json!({"cimd_host": cimd_host, "reason": "abuse"}))
        .await
        .expect("admin.oauth.client_block");
    assert_eq!(common::extract_structured(&block)["blocked"], json!(true));

    // "authorizes": tenant B's own attempt from the same blocked host is
    // refused too -- the block is global, not per-tenant.
    let authorize_after_block = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", cimd_client_id.as_str()),
            ("redirect_uri", "https://blockable.example/callback"),
            ("code_challenge", code_challenge_for("verifier-for-tenant-b-attempt-43-chars-min-ok").as_str()),
            ("code_challenge_method", "S256"),
            ("state", "tenant-b"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key_b.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize after block");
    assert!(authorize_after_block.headers().get("location").is_none(), "a blocked host must never redirect");
    let authorize_body = authorize_after_block.text().await.expect("body");
    assert!(authorize_body.contains("client_blocked"), "authorize must read client_blocked: {authorize_body}");

    // "exchanges": the code minted before the block now refuses too.
    let exchange_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "https://blockable.example/callback"),
            ("client_id", cimd_client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token after block")
        .json()
        .await
        .expect("parse token response");
    assert_eq!(exchange_resp["error_description"], json!("client_blocked"));

    // Every refusal is counted in admin.oauth.stats.
    let stats = admin_client.tools_call("admin.oauth.stats", json!({})).await.expect("admin.oauth.stats");
    let stats_structured = common::extract_structured(&stats);
    assert!(stats_structured["blocks"].as_i64().unwrap() >= 1);
    assert!(
        stats_structured["refused_client_blocked"].as_i64().unwrap() >= 2,
        "authorize and exchange refusals must both be counted: {stats_structured:?}"
    );

    let blocked_list = admin_client.tools_call("admin.oauth.blocked", json!({})).await.expect("admin.oauth.blocked");
    let entries = common::extract_structured(&blocked_list)["blocked"].as_array().cloned().unwrap_or_default();
    assert!(entries.iter().any(|e| e["kind"] == json!("cimd_host") && e["value"] == json!(cimd_host)));
}
