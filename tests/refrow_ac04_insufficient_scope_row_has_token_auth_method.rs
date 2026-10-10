//! AC4 — Given an OAuth token lacking the tool's scope, When it calls the
//! tool, Then the row's `error_code` is `insufficient_scope` and
//! `auth_method` is the token's method.

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
async fn insufficient_scope_refusal_writes_row_with_token_auth_method() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Refused Scope Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    for (name, desc) in [("read", "Search"), ("write", "Change records")] {
        key_client
            .tools_call("host.oauth.scope_set", json!({"name": name, "description": desc}))
            .await
            .expect("scope_set");
    }
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "delete_records", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["write"]}),
        )
        .await
        .expect("publish delete_records");

    let access_token = consent_and_get_token(&server, &ns, &key, "read offline_access").await;
    let token_client = McpClient::with_bearer(&server.base_url, &access_token);
    let err = token_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect_err("a read-scoped token must be refused");
    assert_eq!(err.error_code.as_deref(), Some("insufficient_scope"));

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let rows: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT error_code, auth_method, ok FROM calls \
             WHERE tenant_id = (SELECT id FROM tenants WHERE namespace = ?1) AND error_code IS NOT NULL",
        )
        .expect("prepare")
        .query_map([&ns], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("collect");
    assert_eq!(rows, vec![("insufficient_scope".to_string(), "hosted_token".to_string(), 0)]);
}
