//! PRD-mcphost-owner-verification-paths
//! AC3 (P0) — Given `MCPHOST_GITHUB_CLIENT_ID` set and a stubbed GitHub token
//! endpoint, When the callback arrives with a valid state and code, Then the
//! tenant is verified with `method = github, handle = <login>`; Given an
//! expired or unknown state, Then 400 and nothing is written.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::verify::{GithubConfig, OwnerVerify};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn stubbed_github() -> MockServer {
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
    github
}

fn config_for(github: &MockServer) -> OwnerVerify {
    let mut cfg = GithubConfig::new("cid-ownv", "csecret-ownv");
    cfg.token_url = format!("{}/login/oauth/access_token", github.uri());
    cfg.user_url = format!("{}/user", github.uri());
    OwnerVerify { github: Some(cfg), ..OwnerVerify::default() }
}

fn state_param(authorize_url: &str) -> String {
    let query = authorize_url.split_once('?').expect("query").1;
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("state="))
        .expect("state param")
        .to_string()
}

#[tokio::test]
async fn ownv_ac03_valid_state_and_code_verifies_with_the_github_login() {
    let github = stubbed_github().await;
    let server = TestServer::start_with_owner_verify(config_for(&github)).await;
    let (ns, key) = signup(&server.base_url, "Ownv AC3 Valid").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let started = extract_structured(&client.tools_call("host.claim.github", json!({})).await.expect("host.claim.github"));
    let authorize_url = started["authorize_url"].as_str().expect("authorize_url");
    assert!(authorize_url.contains("client_id=cid%2Downv"), "{authorize_url}");
    assert_eq!(started["expires_in"], 600);
    let oauth_state = state_param(authorize_url);

    let resp = reqwest::get(format!("{}/claim/github/callback?code=abc&state={oauth_state}", server.base_url))
        .await
        .expect("callback");
    assert_eq!(resp.status(), 200);

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    assert!(tenant.owner_verified_at.is_some(), "tenant must be verified");
    let (method, handle) = server.state.db.owner_provenance(tenant.id).await.unwrap();
    assert_eq!(method.as_deref(), Some("github"));
    assert_eq!(handle.as_deref(), Some("octo-owner"));

    // The state is single-use: replaying it is a 400 and changes nothing.
    let replay = reqwest::get(format!("{}/claim/github/callback?code=abc&state={oauth_state}", server.base_url))
        .await
        .expect("replay");
    assert_eq!(replay.status(), 400);
}

#[tokio::test]
async fn ownv_ac03_expired_or_unknown_state_is_400_and_writes_nothing() {
    let github = stubbed_github().await;
    let server = TestServer::start_with_owner_verify(config_for(&github)).await;
    let (ns, _key) = signup(&server.base_url, "Ownv AC3 Expired").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    server.state.db.put_github_state("expired-state".to_string(), tenant.id, 1).await.unwrap();
    for state in ["expired-state", "never-issued"] {
        let resp = reqwest::get(format!("{}/claim/github/callback?code=abc&state={state}", server.base_url))
            .await
            .expect("callback");
        assert_eq!(resp.status(), 400, "state {state}");
    }

    let after = server.state.db.find_tenant_by_namespace(tenant.namespace.clone()).await.unwrap().unwrap();
    assert!(after.owner_verified_at.is_none(), "nothing may be written");
    assert_eq!(server.state.db.owner_provenance(tenant.id).await.unwrap(), (None, None));
    let audit = server.state.db.list_tenant_audit_for_subject(tenant.id, "owner".to_string(), None, 10).await.unwrap();
    assert!(audit.is_empty(), "no tenant_audit row: {audit:?}");
}
