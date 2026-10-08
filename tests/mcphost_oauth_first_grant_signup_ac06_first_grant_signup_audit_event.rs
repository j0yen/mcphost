//! PRD-mcphost-oauth-first-grant-signup
//! AC6 (P0) — Given AC1 completed, When `host.oauth.audit` is read for that
//! tenant, Then one `first_grant_signup` event names the client id;
//! `admin_audit` lists the same event.

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

#[tokio::test]
async fn first_grant_signup_event_is_in_oauth_audit_and_admin_audit() {
    let server = TestServer::start().await;
    let http = http();
    let client_id = register(&http, &server.base_url).await;
    let resp = consent(&http, &server.base_url, &client_id, &[("new_workspace", "1")]).await;
    let access_token = exchange(&http, &server.base_url, &client_id, &resp).await;

    let client = McpClient::with_bearer(&server.base_url, &access_token);
    let audit = common::extract_structured(&client.tools_call("host.oauth.audit", json!({"limit": 50})).await.expect("audit"));
    let events: Vec<&Value> = audit["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|e| e["event"] == json!("first_grant_signup"))
        .collect();
    assert_eq!(events.len(), 1, "exactly one first_grant_signup event: {audit}");
    assert_eq!(events[0]["client_id"], json!(client_id));

    let tenant = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"))["tenant"]
        .as_str()
        .expect("tenant")
        .to_string();
    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let log = common::extract_structured(&admin.tools_call("admin.audit_log", json!({"limit": 100})).await.expect("audit_log"));
    let entries: Vec<&Value> = log["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|e| e["action"] == json!("first_grant_signup"))
        .collect();
    assert_eq!(entries.len(), 1, "admin_audit lists the same event: {log}");
    assert_eq!(entries[0]["target"], json!(tenant));
    assert_eq!(entries[0]["detail"], json!(client_id));
}
