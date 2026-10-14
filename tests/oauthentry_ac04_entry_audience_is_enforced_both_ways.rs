//! PRD-mcphost-oauth-first-entry-url
//! AC4 (P0) — Given a token minted for `<public_url>/mcp` or for
//! `<public_url>/t/<ns>/mcp`, When presented on `/login/mcp`, Then 401 with
//! error `wrong_audience`; Given the entry token presented on `/mcp`, Then
//! `wrong_audience`.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::oauth::{OAUTH_ENTRY_PATH, entry_resource_uri};
use serde_json::{Value, json};

async fn whoami_raw(server: &TestServer, path: &str, token: &str) -> (reqwest::StatusCode, Value) {
    let client = McpClient::with_bearer(&server.base_url, token).with_path(path);
    let resp = client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "host.whoami", "arguments": {}},
            }),
            "host.whoami",
        )
        .await;
    let status = resp.status();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

fn token_of(resp: &Value) -> String {
    resp["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {resp}")).to_string()
}

#[tokio::test]
async fn root_and_per_tenant_tokens_are_wrong_audience_on_the_entry_path() {
    let server = TestServer::start().await;
    let (_ns_root, root_resp) =
        common::oauth_consent_and_token(&server.base_url, &format!("{}/mcp", server.base_url), &[("new_workspace", "1")]).await;
    let root_token = token_of(&root_resp);

    let (ns, key) = common::signup(&server.base_url, "Tenant Resource Owner").await;
    let (_cid, tenant_resp) = common::oauth_consent_and_token(
        &server.base_url,
        &format!("{}/t/{ns}/mcp", server.base_url),
        &[("tenant_key", key.as_str())],
    )
    .await;
    let tenant_token = token_of(&tenant_resp);

    for (label, token) in [("root", root_token), ("per-tenant", tenant_token)] {
        let (status, body) = whoami_raw(&server, OAUTH_ENTRY_PATH, &token).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "{label} token on the entry path: {body}");
        assert_eq!(body["error"]["data"]["error_description"], json!("wrong_audience"), "{label}: {body}");
    }
}

#[tokio::test]
async fn the_entry_token_is_wrong_audience_on_root_and_on_a_tenant_path_but_fine_on_the_entry_path() {
    let server = TestServer::start().await;
    let (_cid, resp) =
        common::oauth_consent_and_token(&server.base_url, &entry_resource_uri(&server.base_url), &[("new_workspace", "1")]).await;
    let entry_token = token_of(&resp);

    let (status, body) = whoami_raw(&server, "/mcp", &entry_token).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "entry token on /mcp: {body}");
    assert_eq!(body["error"]["data"]["error_description"], json!("wrong_audience"), "{body}");

    let (ns, _key) = common::signup(&server.base_url, "Other Tenant").await;
    let (status, body) = whoami_raw(&server, &format!("/t/{ns}/mcp"), &entry_token).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "entry token on a tenant path: {body}");

    let (status, body) = whoami_raw(&server, OAUTH_ENTRY_PATH, &entry_token).await;
    assert_eq!(status, reqwest::StatusCode::OK, "entry token on the entry path: {body}");
}

#[tokio::test]
async fn a_malformed_or_non_oauth_bearer_on_the_entry_path_is_401_not_served() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Key Holder").await;
    for token in ["not-a-jwt", key.as_str()] {
        let (status, body) = whoami_raw(&server, OAUTH_ENTRY_PATH, token).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "{body}");
    }
}
