//! PRD-mcphost-oauth-first-entry-url
//! AC5 (P0) — Given the conformance harness profiles `claude` and `chatgpt`
//! with no pre-existing tenant, When run against the entry URL, Then both
//! reach `tools/list` through "new workspace".

use crate::common;
use common::{McpClient, TestServer};
use mcphost::oauth::OAUTH_ENTRY_PATH;
use mcphost::oauthclient::{ClientKind, Verdict, build_authorize_request, discover_401, read_prm};
use serde_json::{Value, json};

/// One harness profile from nothing: the 401 challenge on the entry URL, the
/// entry document it names, DCR with the profile's redirect, the harness's
/// own authorize request against the document's `resource`, the
/// new-workspace consent, the token exchange, then `tools/list` on the entry
/// path.
async fn run_profile(server: &TestServer, name: &str) {
    let kind = ClientKind::parse(name).expect("known profile");
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let entry_url = format!("{}{OAUTH_ENTRY_PATH}", server.base_url);

    let (step, discovered) = discover_401(&http, &entry_url).await;
    assert!(matches!(step.verdict, Verdict::Pass), "{name}: discover_401 must pass on the entry URL: {step:?}");
    let metadata_url = discovered.and_then(|d| d.resource_metadata_url).expect("resource_metadata url");
    let (step, prm) = read_prm(&http, &metadata_url).await;
    let prm = prm.unwrap_or_else(|| panic!("{name}: read_prm failed: {step:?}"));
    assert_eq!(prm.resource, entry_url, "{name}: the document names the entry resource");

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

    let req = build_authorize_request(
        &format!("{}/oauth/authorize", server.base_url),
        &client_id,
        kind,
        &prm.resource,
        Some("mcp"),
        &[],
    );
    let page = http.get(&req.url).send().await.expect("GET authorize").text().await.expect("page");
    assert!(page.contains("Create a new workspace"), "{name}: consent page offers the new choice: {page}");

    let mut form: Vec<(String, String)> = req.params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    form.push(("new_workspace".to_string(), "1".to_string()));
    let consent = http.post(format!("{}/oauth/authorize", server.base_url)).form(&form).send().await.expect("consent");
    assert!(consent.status().is_redirection(), "{name}: consent must redirect: {}", consent.status());
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("query");
    let code = query.split('&').find_map(|p| p.strip_prefix("code=")).expect("code").to_string();

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
        .with_path(OAUTH_ENTRY_PATH)
        .tools_list()
        .await
        .unwrap_or_else(|e| panic!("{name}: tools/list on the entry path failed: {e:?}"));
    assert!(listed["tools"].as_array().is_some_and(|t| !t.is_empty()), "{name}: tools/list must return tools: {listed}");
}

#[tokio::test]
async fn claude_and_chatgpt_profiles_reach_tools_list_through_new_workspace_on_the_entry_url() {
    let server = TestServer::start().await;
    assert!(server.state.db.list_tenants().await.expect("list tenants").is_empty(), "no pre-existing tenant");
    run_profile(&server, "claude").await;
    run_profile(&server, "chatgpt").await;
}
