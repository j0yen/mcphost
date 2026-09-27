//! PRD-mcphost-tool-scopes-and-consent
//! AC1 (P0) — Given tenant T with catalog `{read: "Search", write: "Change
//! records"}`, tool `search {scopes:["read"]}` and tool `delete_records
//! {scopes:["write"]}`, When a client authorizes with `scope=read
//! offline_access`, Then the consent page lists `read` with "Search" and
//! the tool `search` under it, and the issued token's `scope` is `read
//! offline_access`.

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

#[tokio::test]
async fn consent_page_groups_by_scope_and_token_carries_requested_scope() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Scoped Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    key_client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("host.oauth.scope_set read must succeed");
    key_client
        .tools_call(
            "host.oauth.scope_set",
            json!({"name": "write", "description": "Change records"}),
        )
        .await
        .expect("host.oauth.scope_set write must succeed");

    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["read"]}),
        )
        .await
        .expect("publish search must succeed");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "delete_records", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["write"]}),
        )
        .await
        .expect("publish delete_records must succeed");

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

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, &ns);

    let authorize_form = [
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", "http://127.0.0.1/cb"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "abc"),
        ("scope", "read offline_access"),
        ("resource", resource.as_str()),
    ];

    // GET renders the consent page grouped by requested scope -- assert
    // before ever proving tenant ownership, since `resource` alone already
    // names which tenant's catalog governs this consent (requirement 2).
    let mut get_url = reqwest::Url::parse(&format!("{}/oauth/authorize", server.base_url)).unwrap();
    {
        let mut pairs = get_url.query_pairs_mut();
        for (k, v) in &authorize_form {
            pairs.append_pair(k, v);
        }
    }
    let get_resp = http.get(get_url).send().await.expect("GET /oauth/authorize");
    assert!(get_resp.status().is_success(), "consent page must render: {}", get_resp.status());
    let page = get_resp.text().await.expect("consent page body");
    let read_start = page.find("data-scope=\"read\"").expect("consent page must have a read scope block");
    let read_end = page[read_start..].find("</div>").map(|i| read_start + i).unwrap_or(page.len());
    let read_block = &page[read_start..read_end];
    assert!(read_block.contains("Search"), "read block must show its catalog description: {read_block}");
    assert!(read_block.contains("search"), "read block must list the search tool: {read_block}");
    assert!(
        !page.contains("data-scope=\"write\""),
        "only the requested scope (read) may be grouped, not write: {page}"
    );

    // POST proves ownership with the tenant key and mints a code.
    let mut form = authorize_form.to_vec();
    form.push(("tenant_key", key.as_str()));
    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&form)
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
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");

    assert_eq!(token_resp["scope"], json!("read offline_access"));
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();

    let payload_b64 = access_token.split('.').nth(1).expect("JWT has a payload segment");
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).expect("base64url decode payload");
    let claims: Value = serde_json::from_slice(&payload_bytes).expect("parse JWT claims");
    assert_eq!(claims["scope"], json!("read offline_access"));
    assert_eq!(claims["sub"], json!(ns));
    assert_eq!(claims["aud"], json!(resource));
}
