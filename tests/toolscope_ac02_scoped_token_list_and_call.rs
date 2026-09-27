//! PRD-mcphost-tool-scopes-and-consent
//! AC2 (P0) — Given that token, When `tools/list` runs, Then only `search`
//! (and scope-less tools) appear; When `tools/call delete_records` runs,
//! Then 403 with `error="insufficient_scope"`, `scope="read write"` and a
//! JSON-RPC error naming `delete_records`.

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
async fn scoped_token_lists_only_permitted_tools_and_403s_the_rest() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Scoped Tenant Two").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    key_client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("scope_set read");
    key_client
        .tools_call(
            "host.oauth.scope_set",
            json!({"name": "write", "description": "Change records"}),
        )
        .await
        .expect("scope_set write");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["read"]}),
        )
        .await
        .expect("publish search");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "delete_records", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["write"]}),
        )
        .await
        .expect("publish delete_records");

    let access_token = consent_and_get_token(&server, &ns, &key, "read offline_access").await;
    let token_client = McpClient::with_bearer(&server.base_url, &access_token);

    let listed = token_client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&format!("{ns}.search").as_str()),
        "read-scoped token must see search: {names:?}"
    );
    assert!(
        !names.contains(&format!("{ns}.delete_records").as_str()),
        "read-scoped token must not see delete_records: {names:?}"
    );
    assert!(
        names.contains(&"host.whoami"),
        "scope-less host.* control-plane tools must still be listed: {names:?}"
    );

    let err = token_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect_err("delete_records must be refused for a read-scoped token");
    assert_eq!(err.error_code.as_deref(), Some("insufficient_scope"));
    assert_eq!(err.data["scope"], json!("read write"));
    assert_eq!(err.data["tool"], json!("delete_records"));

    // Same call, over raw HTTP, to check the 403 + WWW-Authenticate
    // challenge (requirement 4) -- same "omit MCP-Protocol-Version, send
    // Mcp-Name directly" convention `tenantprm_ac05`'s own `bare_call`
    // helper uses.
    let name = format!("{ns}.delete_records");
    let resp = token_client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": {}},
            }),
            &name,
        )
        .await;
    let status = resp.status();
    let header = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN, "header: {header}");
    assert!(header.contains("error=\"insufficient_scope\""), "{header}");
    assert!(header.contains("scope=\"read write\""), "{header}");
}
