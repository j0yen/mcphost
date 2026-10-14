//! PRD-mcphost-oauth-metadata-lists-hosted-as
//! AC5 (P0) — Given a fresh key-signup tenant on a test server, When the
//! `oauthmeta` contract walk runs from its tenant document with no
//! hard-coded authorization-server URL, Then `discover`, `metadata`,
//! `register`, `authorize`, `consent`, and `token` succeed and a bearer
//! `tools/call` on the document's `resource` returns the tenant's
//! `host.whoami`.
//!
//! Every URL after the first is read from the previous document: the
//! tenant's RFC 9728 document names the authorization server, that
//! server's RFC 8414 document names the endpoints, and the token is used on
//! the `resource` the first document named.

use crate::common;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

#[tokio::test]
async fn walk_from_tenant_document_to_bearer_tools_call() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Contract Walk Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let mut steps: Vec<&str> = Vec::new();

    // discover: the only URL the walk is given is the tenant document's.
    let prm_url = format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url);
    let prm: Value = http.get(&prm_url).send().await.expect("GET tenant document").json().await.expect("PRM JSON");
    let resource = prm["resource"].as_str().expect("document names a resource").to_string();
    let as_url = prm["authorization_servers"][0].as_str().expect("document lists an authorization server").to_string();
    steps.push("discover");

    // metadata: the authorization server's own document names its endpoints.
    let as_meta: Value = http
        .get(format!("{}/.well-known/oauth-authorization-server", as_url.trim_end_matches('/')))
        .send()
        .await
        .expect("GET AS metadata")
        .json()
        .await
        .expect("AS metadata JSON");
    let registration_endpoint = as_meta["registration_endpoint"].as_str().expect("registration_endpoint").to_string();
    let authorization_endpoint = as_meta["authorization_endpoint"].as_str().expect("authorization_endpoint").to_string();
    let token_endpoint = as_meta["token_endpoint"].as_str().expect("token_endpoint").to_string();
    steps.push("metadata");

    // register: a native DCR client.
    let redirect_uri = "http://127.0.0.1/cb";
    let register: Value = http
        .post(&registration_endpoint)
        .json(&json!({"application_type": "native", "redirect_uris": [redirect_uri]}))
        .send()
        .await
        .expect("POST registration")
        .json()
        .await
        .expect("register JSON");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();
    steps.push("register");

    let verifier = "an-oauthmeta-pkce-verifier-of-realistic-length-0123456789";
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let params = [
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "walk"),
        ("scope", "mcp"),
        ("resource", resource.as_str()),
    ];

    // authorize: the consent page renders for the document's own resource.
    let page = http.get(&authorization_endpoint).query(&params).send().await.expect("GET authorize");
    assert!(page.status().is_success(), "authorize must render the consent page: {}", page.status());
    steps.push("authorize");

    // consent: prove tenant ownership with the tenant key.
    let mut form: Vec<(&str, &str)> = params.to_vec();
    form.push(("tenant_key", key.as_str()));
    let consent = http.post(&authorization_endpoint).form(&form).send().await.expect("POST consent");
    assert!(consent.status().is_redirection(), "consent must redirect: {}", consent.status());
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect carries a query");
    assert_eq!(query_param(query, "state"), Some("walk"));
    let code = query_param(query, "code").expect("authorization code").to_string();
    steps.push("consent");

    // token
    let token: Value = http
        .post(&token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("POST token")
        .json()
        .await
        .expect("token JSON");
    let access_token = token["access_token"].as_str().unwrap_or_else(|| panic!("access_token in {token}")).to_string();
    steps.push("token");

    assert_eq!(steps, ["discover", "metadata", "register", "authorize", "consent", "token"]);

    // tools/call on the document's resource, bearer = the minted token.
    let path = resource.strip_prefix(server.base_url.as_str()).expect("resource lives under the public URL");
    let bearer = McpClient::with_bearer(&server.base_url, &access_token).with_path(path);
    let whoami = common::extract_structured(
        &bearer.tools_call("host.whoami", json!({})).await.expect("host.whoami over the bearer"),
    );
    assert_eq!(whoami["tenant"], json!(ns), "the bearer must run as the tenant that consented: {whoami}");
    assert_eq!(whoami["auth_method"], json!("hosted_token"));
}
