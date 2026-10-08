//! AC6 (PRD-mcphost-chatgpt-submission-pack) — Given the reviewer notes, When
//! the verifier follows them literally against a fixture server from a fresh
//! session, Then the first `host.quickstart kind=echo` call succeeds with no
//! credential typed.
//!
//! "Literally" means: the consent choice and the tool call are read out of
//! the committed `submission/chatgpt/reviewer-notes.md`, not retyped here, so
//! a notes edit that names a different button or call changes what runs.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::oauthclient::{ClientKind, build_authorize_request};
use serde_json::{Value, json};

/// Drives one conformance-harness client profile end to end against the real
/// host: DCR with the profile's redirect + application type, the harness's own
/// authorize request, the consent form's new-workspace choice, the token
/// exchange, then `tools/list`.
async fn run_profile(server: &TestServer, name: &str) -> String {
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
    let access_token = token["access_token"].as_str().unwrap_or_else(|| panic!("{name}: {token}")).to_string();

    let listed = McpClient::with_bearer(&server.base_url, &access_token)
        .tools_list()
        .await
        .unwrap_or_else(|e| panic!("{name}: tools/list failed: {e:?}"));
    assert!(
        listed["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "{name}: tools/list must return tools: {listed}"
    );
    access_token
}

/// `call host.quickstart kind=echo` -> ("host.quickstart", {"kind": "echo"}).
fn call_from_notes(notes: &str) -> (String, Value) {
    let rest = notes.split("call ").nth(1).expect("notes contain a `call <tool> <k=v>` instruction");
    let mut words = rest.split_whitespace();
    let tool = words.next().expect("tool name").to_string();
    let mut args = serde_json::Map::new();
    for w in words {
        let w = w.trim_end_matches('.');
        let Some((k, v)) = w.split_once('=') else { break };
        args.insert(k.to_string(), json!(v));
    }
    (tool, Value::Object(args))
}

#[tokio::test]
async fn following_the_reviewer_notes_literally_reaches_a_working_first_call_with_no_credential() {
    let notes = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/submission/chatgpt/reviewer-notes.md"))
        .expect("reviewer-notes.md");
    assert!(notes.contains("Create a new workspace"), "notes name the consent choice: {notes}");
    assert!(!notes.contains("Bearer") && !notes.contains("Authorization"), "no credential to type: {notes}");
    let (tool, args) = call_from_notes(&notes);
    assert_eq!((tool.as_str(), &args), ("host.quickstart", &json!({"kind": "echo"})), "{notes}");

    // Fresh session: empty tenant table, no key anywhere; the only way in is the notes' path.
    let server = TestServer::start().await;
    assert!(server.state.db.list_tenants().await.expect("list tenants").is_empty());
    let access_token = run_profile(&server, "chatgpt").await;

    let result = McpClient::with_bearer(&server.base_url, &access_token)
        .tools_call(&tool, args)
        .await
        .unwrap_or_else(|e| panic!("first call from the notes failed: {e:?}"));
    let structured = common::extract_structured(&result);
    assert!(structured.get("error").is_none(), "{structured}");
    assert!(structured.as_object().is_some_and(|o| !o.is_empty()), "quickstart returned content: {structured}");
    assert!(result["isError"] != json!(true), "{result}");
}
