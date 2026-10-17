//! PRD-mcphost-owner-verification-paths AC4 (P0) -- Given the GitHub env
//! unset, When `host.claim.github` runs, Then `method_unavailable {method:
//! "github"}` and `/claim/methods.json` still lists `github` with
//! `available: false`.

use crate::common;
use common::{EnvGuardMany, McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn ownv_ac04() {
    let _env = EnvGuardMany::set(&[("MCPHOST_GITHUB_CLIENT_ID", None), ("MCPHOST_GITHUB_CLIENT_SECRET", None)]).await;
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ownv AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.claim.github", json!({}))
        .await
        .expect_err("no GitHub app configured");
    assert_eq!(err.error_code.as_deref(), Some("method_unavailable"), "{err:?}");
    assert_eq!(err.data["method"], json!("github"), "{err:?}");

    let body: Value = reqwest::get(format!("{}/claim/methods.json", server.base_url))
        .await
        .expect("GET methods.json")
        .json()
        .await
        .expect("json");
    let github = body["methods"]
        .as_array()
        .expect("methods")
        .iter()
        .find(|m| m["name"] == json!("github"))
        .unwrap_or_else(|| panic!("github still listed: {body}"));
    assert_eq!(github["available"], json!(false), "{body}");
    let dns = body["methods"].as_array().expect("methods").iter().find(|m| m["name"] == json!("dns")).expect("dns");
    assert_eq!(dns["available"], json!(true), "{body}");
}
