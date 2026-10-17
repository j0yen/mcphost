//! PRD-mcphost-owner-verification-paths AC3 (P0) -- Given
//! `MCPHOST_GITHUB_CLIENT_ID` set and a stubbed GitHub token endpoint, When
//! the callback arrives with a valid state and code, Then the tenant is
//! verified with `method = github, handle = <login>`; Given an expired or
//! unknown state, Then 400 and nothing is written.

use crate::common;
use common::{EnvGuardMany, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn query_param(url: &str, name: &str) -> String {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no '{name}' param in {url}"))
        .to_string()
}

async fn audit_rows(server: &TestServer, ns: &str) -> i64 {
    let tenant = server.state.db.find_tenant_by_namespace(ns.to_string()).await.expect("db").expect("tenant");
    server
        .state
        .db
        .count_tenant_audit_for_test(tenant.id, "claim.github".to_string())
        .await
        .expect("audit count")
}

#[tokio::test]
async fn ownv_ac03() {
    let github = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "gho_stub", "token_type": "bearer"})))
        .mount(&github)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "octo-owner", "id": 4242})))
        .mount(&github)
        .await;
    let _env = EnvGuardMany::set(&[
        ("MCPHOST_GITHUB_CLIENT_ID", Some("stub-client-id")),
        ("MCPHOST_GITHUB_CLIENT_SECRET", Some("stub-client-secret")),
        ("MCPHOST_GITHUB_BASE_URL", Some(&github.uri())),
        ("MCPHOST_GITHUB_API_URL", Some(&github.uri())),
    ])
    .await;

    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ownv AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let http = reqwest::Client::new();
    let callback = |state: &str| format!("{}/claim/github/callback?code=the-code&state={state}", server.base_url);

    // Unknown state: 400, nothing written.
    let resp = http.get(callback("not-a-real-state")).send().await.expect("callback");
    assert_eq!(resp.status(), 400);
    let me = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["owner_verified"], json!(false), "{me}");
    assert_eq!(audit_rows(&server, &ns).await, 0);

    // Expired state: 400, nothing written.
    let started = extract_structured(&client.tools_call("host.claim.github", json!({})).await.expect("claim.github"));
    let authorize_url = started["authorize_url"].as_str().expect("authorize_url").to_string();
    assert!(authorize_url.starts_with(&github.uri()), "{authorize_url}");
    assert_eq!(query_param(&authorize_url, "client_id"), "stub-client-id");
    assert_eq!(started["expires_in"], json!(600));
    let expired_state = query_param(&authorize_url, "state");
    server.state.db.expire_github_claim_states_for_test().await.expect("expire");
    let resp = http.get(callback(&expired_state)).send().await.expect("callback");
    assert_eq!(resp.status(), 400);
    let me = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["owner_verified"], json!(false), "{me}");
    assert_eq!(audit_rows(&server, &ns).await, 0);

    // Valid state + code: verified as github, handle = login.
    let started = extract_structured(&client.tools_call("host.claim.github", json!({})).await.expect("claim.github"));
    let state = query_param(started["authorize_url"].as_str().expect("authorize_url"), "state");
    let resp = http.get(callback(&state)).send().await.expect("callback");
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap_or_default());
    let me: Value = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["owner_verified"], json!(true), "{me}");
    assert_eq!(me["owner"]["method"], json!("github"), "{me}");
    assert_eq!(me["owner"]["handle"], json!("octo-owner"), "{me}");
    assert_eq!(audit_rows(&server, &ns).await, 1);

    // A used state is single-use.
    let resp = http.get(callback(&state)).send().await.expect("callback");
    assert_eq!(resp.status(), 400);
}
