//! PRD-mcphost-scope-registry-generated
//! AC3 (P0) — Given a token with scope `tools:write` and a tool requiring
//! `tools:read`, When it calls, Then the call succeeds; Given scope
//! `tools:read` and a tool requiring `tools:write`, Then the refusal code
//! equals `scopes::INSUFFICIENT_SCOPE` and `data.scope` lists the union.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

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

/// Runs the full consent+token flow (same shape as AC1's own test) for a
/// tenant with a `{read, write}` catalog and `search`/`delete_records`
/// tools, returning the hosted access token minted for `scope`.
async fn consent_and_get_token(server: &TestServer, ns: &str, key: &str, scope: &str) -> String {
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();

    let verifier = format!("verifier-{scope}-at-least-43-chars-long-for-pkce-realism");
    let challenge = code_challenge_for(&verifier);
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, ns);

    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", scope),
            ("resource", resource.as_str()),
            ("tenant_key", key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect must carry a query string");
    let code = query_param(query, "code").expect("code present in redirect").to_string();

    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    token_resp["access_token"].as_str().expect("access_token present").to_string()
}

#[tokio::test]
async fn sreg_ac03_write_implies_read_but_read_does_not_imply_write() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Scope Implies Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    for (name, scope) in [("peek", "tools:read"), ("mutate", "tools:write")] {
        key_client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": [scope]}),
            )
            .await
            .expect("publish");
    }

    let write_token = consent_and_get_token(&server, &ns, &key, "tools:write").await;
    let write_client = McpClient::with_bearer(&server.base_url, &write_token);
    write_client
        .tools_call(&format!("{ns}.peek"), json!({}))
        .await
        .expect("tools:write satisfies a tools:read tool");

    let read_token = consent_and_get_token(&server, &ns, &key, "tools:read").await;
    let read_client = McpClient::with_bearer(&server.base_url, &read_token);
    let err = read_client
        .tools_call(&format!("{ns}.mutate"), json!({}))
        .await
        .expect_err("tools:read must not satisfy a tools:write tool");
    assert_eq!(err.error_code.as_deref(), Some(mcphost::scopes::INSUFFICIENT_SCOPE));
    assert_eq!(err.data["scope"], json!("tools:read tools:write"));
}

#[test]
fn sreg_ac03_implication_table() {
    use mcphost::scopes::Scope;
    assert!(Scope::ToolsWrite.implies(Scope::ToolsRead));
    assert!(Scope::ToolsCall.implies(Scope::ToolsRead));
    assert!(!Scope::ToolsRead.implies(Scope::ToolsWrite));
    assert!(Scope::StateWrite.implies(Scope::StateRead));
    assert!(Scope::ALL.iter().all(|s| Scope::Mcp.implies(*s)));
}
