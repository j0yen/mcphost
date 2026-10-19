//! PRD-mcphost-owner-verification-paths
//! AC4 (P0) — Given the GitHub env unset, When `host.claim.github` runs, Then
//! `method_unavailable {method: "github"}` and `/claim/methods.json` still
//! lists `github` with `available: false`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::verify::OwnerVerify;
use serde_json::{Value, json};

#[test]
fn ownv_ac04_unset_env_builds_no_github_config() {
    let unset = OwnerVerify::from_lookup(|_| None);
    assert!(unset.github.is_none());
    let id_only = OwnerVerify::from_lookup(|n| (n == "MCPHOST_GITHUB_CLIENT_ID").then(|| "cid".to_string()));
    assert!(id_only.github.is_none(), "a client id without its secret is not a usable app");
    let both = OwnerVerify::from_lookup(|n| match n {
        "MCPHOST_GITHUB_CLIENT_ID" => Some("cid".to_string()),
        "MCPHOST_GITHUB_CLIENT_SECRET" => Some("secret".to_string()),
        _ => None,
    });
    assert_eq!(both.github.expect("configured").client_id, "cid");
}

#[tokio::test]
async fn ownv_ac04_claim_github_is_method_unavailable_and_listed_unavailable() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ownv AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client.tools_call("host.claim.github", json!({})).await.expect_err("method_unavailable");
    assert_eq!(err.error_code.as_deref(), Some("method_unavailable"), "{}", err.message);
    assert_eq!(err.data["method"], "github", "{}", err.data);

    let doc: Value = reqwest::get(format!("{}/claim/methods.json", server.base_url))
        .await
        .expect("GET")
        .json()
        .await
        .expect("json");
    let github = doc["methods"]
        .as_array()
        .expect("methods")
        .iter()
        .find(|m| m["name"] == "github")
        .expect("github still listed");
    assert_eq!(github["available"], false);
    assert_eq!(github["tool"], "host.claim.github");
}
