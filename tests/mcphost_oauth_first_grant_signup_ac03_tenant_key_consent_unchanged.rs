//! PRD-mcphost-oauth-first-grant-signup
//! AC3 (P0) — Given a consent form posted with a valid `tenant_key`, When
//! submitted, Then behaviour is byte-identical to the current build (existing
//! suites pass unchanged).

use crate::common;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{McpClient, TestServer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERIFIER: &str = "a-pkce-verifier-at-least-43-chars-long-for-realism";

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

async fn register(http: &reqwest::Client, base_url: &str) -> String {
    let body: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("register")
        .json()
        .await
        .expect("register json");
    body["client_id"].as_str().expect("client_id").to_string()
}

/// POSTs the consent form with `extra` fields appended.
async fn consent(http: &reqwest::Client, base_url: &str, client_id: &str, extra: &[(&str, &str)]) -> reqwest::Response {
    let resource = format!("{base_url}/mcp");
    let challenge = code_challenge_for(VERIFIER);
    let mut form = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", "http://127.0.0.1/cb"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "abc"),
        ("scope", "mcp"),
        ("resource", resource.as_str()),
    ];
    form.extend_from_slice(extra);
    http.post(format!("{base_url}/oauth/authorize")).form(&form).send().await.expect("consent")
}

async fn exchange(http: &reqwest::Client, base_url: &str, client_id: &str, resp: &reqwest::Response) -> String {
    let location = resp.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("query");
    let code = query.split('&').find_map(|p| p.strip_prefix("code=")).expect("code").to_string();
    let token: Value = http
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .expect("token")
        .json()
        .await
        .expect("token json");
    token["access_token"].as_str().unwrap_or_else(|| panic!("{token}")).to_string()
}

async fn tenant_count(server: &TestServer) -> usize {
    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let out = admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants");
    common::extract_structured(&out)["tenants"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn tenant_key_consent_is_unchanged_and_creates_no_tenant() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Keyed Tenant").await;
    let http = http();
    let client_id = register(&http, &server.base_url).await;
    let before = tenant_count(&server).await;

    // Even with the new-workspace button also present, the key wins.
    let resp = consent(&http, &server.base_url, &client_id, &[("tenant_key", key.as_str()), ("new_workspace", "1")]).await;
    assert!(resp.status().is_redirection(), "key consent must redirect: {}", resp.status());
    let access_token = exchange(&http, &server.base_url, &client_id, &resp).await;
    assert_eq!(tenant_count(&server).await, before, "a keyed consent must not create a tenant");

    let client = McpClient::with_bearer(&server.base_url, &access_token);
    let who = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert!(who["source"].is_null(), "an existing tenant keeps its own source: {who}");
    assert!(who.get("onboarding").is_none(), "no onboarding for a keyed grant: {who}");
    let audit = common::extract_structured(&client.tools_call("host.oauth.audit", json!({"limit": 50})).await.expect("audit"));
    assert!(
        !audit["events"].as_array().cloned().unwrap_or_default().iter().any(|e| e["event"] == json!("first_grant_signup")),
        "no first_grant_signup event for a keyed grant: {audit}"
    );

    // A blank/wrong credential with no new-workspace choice still re-renders.
    let bad = consent(&http, &server.base_url, &client_id, &[("tenant_key", "wrong")]).await;
    assert_eq!(bad.status(), reqwest::StatusCode::OK);
    assert!(bad.text().await.unwrap().contains("did not verify"));
    assert_eq!(tenant_count(&server).await, before);
}
