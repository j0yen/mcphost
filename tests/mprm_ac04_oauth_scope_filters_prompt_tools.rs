//! AC4 (P0) -- PRD-mcphost-prompts-from-published-tools. An OAuth session
//! scoped `tools:read` sees the recipes and the prompt-kind tool that
//! requires that scope; one scoped `tables:read` only sees the recipes --
//! the filter is the one `tools/list` applies.

use crate::common;
use crate::promptkind;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{McpClient, TestServer, signup};
use promptkind::{PromptKind, classify_spec};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Consent + token exchange for `scope` on tenant `ns`.
async fn token_for(server: &TestServer, ns: &str, key: &str, scope: &str) -> String {
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("register")
        .json()
        .await
        .expect("register json");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();
    let verifier = format!("verifier-{scope}-at-least-43-chars-long-for-pkce-realism");
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, ns);
    let consent = http
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
        .expect("authorize");
    assert!(consent.status().is_redirection(), "consent must redirect: {}", consent.status());
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect query");
    let code = query_param(query, "code").expect("code").to_string();
    let token: Value = http
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
        .expect("token")
        .json()
        .await
        .expect("token json");
    token["access_token"].as_str().expect("access_token").to_string()
}

fn names(listed: &Value) -> Vec<String> {
    listed["result"]["prompts"]
        .as_array()
        .unwrap_or_else(|| panic!("no prompts: {listed}"))
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn mprm_ac04_oauth_scope_filters_prompt_tools() {
    let kind = PromptKind::new();
    let server = TestServer::start_with_kinds(kind.registry()).await;
    let (ns, key) = signup(&server.base_url, "mprm ac04").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    for scope in ["tools:read", "tables:read"] {
        owner
            .tools_call("host.oauth.scope_set", json!({"name": scope, "description": scope}))
            .await
            .unwrap_or_else(|e| panic!("scope_set {scope}: {e:?}"));
    }
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "classify_ticket", "kind": "prompt", "spec": classify_spec(), "scopes": ["tools:read"]}),
        )
        .await
        .expect("publish");

    let tools_token = token_for(&server, &ns, &key, "tools:read").await;
    let listed = McpClient::with_bearer(&server.base_url, &tools_token).rpc("prompts/list", json!({})).await;
    let got = names(&listed);
    assert!(got.contains(&"classify_ticket".to_string()), "tools:read sees the tool: {got:?}");
    assert!(got.contains(&"webhook-inbox".to_string()), "and the recipes: {got:?}");

    let tables_token = token_for(&server, &ns, &key, "tables:read").await;
    let listed = McpClient::with_bearer(&server.base_url, &tables_token).rpc("prompts/list", json!({})).await;
    let got = names(&listed);
    assert!(!got.contains(&"classify_ticket".to_string()), "tables:read must not see the tool: {got:?}");
    assert!(got.contains(&"webhook-inbox".to_string()), "recipes only: {got:?}");

    // prompts/get applies the same filter: the tool is not renderable for tables:read.
    let denied = McpClient::with_bearer(&server.base_url, &tables_token)
        .rpc("prompts/get", json!({"name": "classify_ticket", "arguments": {"text": "x"}}))
        .await;
    assert_eq!(denied["error"]["code"], -32602, "{denied}");
}
