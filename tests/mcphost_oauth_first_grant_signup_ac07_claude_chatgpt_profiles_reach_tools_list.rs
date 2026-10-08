//! PRD-mcphost-oauth-first-grant-signup
//! AC7 (P0) — Given the OAuth conformance harness profiles `claude` and
//! `chatgpt`, When run against this build with no pre-existing tenant, Then
//! both complete consent through the new choice and reach `tools/list`.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::oauthclient::{ClientKind, build_authorize_request};
use serde_json::{Value, json};

/// Drives one conformance-harness client profile end to end against the real
/// host: DCR with the profile's redirect + application type, the harness's own
/// authorize request, the consent form's new-workspace choice, the token
/// exchange, then `tools/list`.
async fn run_profile(server: &TestServer, name: &str) {
    let kind = ClientKind::parse(name).expect("known profile");
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({
            "application_type": kind.application_type(),
            "redirect_uris": [kind.redirect_uri()],
            "client_name": name,
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .expect("register")
        .json()
        .await
        .expect("register json");
    let client_id = register["client_id"].as_str().unwrap_or_else(|| panic!("{name}: {register}")).to_string();

    let resource = format!("{}/mcp", server.base_url);
    let req = build_authorize_request(
        &format!("{}/oauth/authorize", server.base_url),
        &client_id,
        kind,
        &resource,
        Some("mcp"),
        &[],
    );
    let page = http.get(&req.url).send().await.expect("GET authorize").text().await.expect("page");
    assert!(page.contains("Create a new workspace"), "{name}: consent page offers the new choice: {page}");

    let mut form: Vec<(String, String)> = req.params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    form.push(("new_workspace".to_string(), "1".to_string()));
    let consent = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&form)
        .send()
        .await
        .expect("consent");
    assert!(consent.status().is_redirection(), "{name}: consent must redirect: {}", consent.status());
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    assert!(location.starts_with(kind.redirect_uri()), "{name}: redirect goes to the profile's URI: {location}");
    let (_, query) = location.split_once('?').expect("query");
    let code = query.split('&').find_map(|p| p.strip_prefix("code=")).expect("code").to_string();
    assert!(query.contains(&format!("state={}", req.state)), "{name}: state echoed: {query}");

    let token: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", kind.redirect_uri()),
            ("client_id", client_id.as_str()),
            ("code_verifier", req.code_verifier.as_str()),
        ])
        .send()
        .await
        .expect("token")
        .json()
        .await
        .expect("token json");
    let access_token = token["access_token"].as_str().unwrap_or_else(|| panic!("{name}: {token}"));

    let listed = McpClient::with_bearer(&server.base_url, access_token)
        .tools_list()
        .await
        .unwrap_or_else(|e| panic!("{name}: tools/list failed: {e:?}"));
    assert!(
        listed["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "{name}: tools/list must return tools: {listed}"
    );
}

#[tokio::test]
async fn claude_and_chatgpt_profiles_complete_consent_via_new_workspace_and_list_tools() {
    let server = TestServer::start().await;
    run_profile(&server, "claude").await;
    run_profile(&server, "chatgpt").await;
}
