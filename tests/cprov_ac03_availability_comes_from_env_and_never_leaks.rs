//! PRD-mcphost-connection-provider-registry AC3: with
//! `MCPHOST_CONNECT_GITHUB_CLIENT_ID` set and
//! `MCPHOST_CONNECT_GITHUB_CLIENT_SECRET` unset, `host.connect.providers`
//! reports `github` as `available: false, reason:
//! "client_secret_not_configured"`, and the response carries neither env
//! value. The server reads the env once at startup, so the guard is held
//! across `TestServer::start`.

use crate::common;
use common::{EnvGuard, McpClient, TestServer, extract_structured, signup};
use mcphost::connect::{ConnectProviders, all_specs, providers_json};

const ID_SENTINEL: &str = "cid-sentinel-ac03-7f3a91";

#[tokio::test]
async fn github_with_id_but_no_secret_is_unavailable_with_reason_and_leaks_nothing() {
    let _id = EnvGuard::set("MCPHOST_CONNECT_GITHUB_CLIENT_ID", ID_SENTINEL).await;
    assert!(
        std::env::var("MCPHOST_CONNECT_GITHUB_CLIENT_SECRET").is_err(),
        "precondition: the secret env var is unset"
    );
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.connect.providers", serde_json::json!({}))
        .await
        .expect("host.connect.providers ok");
    let body = extract_structured(&result);
    let github = body["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .find(|p| p["name"] == "github")
        .expect("github row");
    assert_eq!(github["available"], false, "{github}");
    assert_eq!(github["reason"], "client_secret_not_configured", "{github}");

    let raw = result.to_string();
    assert!(!raw.contains(ID_SENTINEL), "client id leaked: {raw}");
}

/// The other half, without touching process env: both vars present and
/// non-empty make a provider available, an empty one does not, and no value
/// ever appears in the rendered rows.
#[test]
fn both_vars_present_is_available_and_empty_counts_as_missing() {
    let lookup = |k: &str| match k {
        "MCPHOST_CONNECT_GOOGLE_CLIENT_ID" => Some("gid-sentinel-ac03".to_string()),
        "MCPHOST_CONNECT_GOOGLE_CLIENT_SECRET" => Some("gsecret-sentinel-ac03".to_string()),
        "MCPHOST_CONNECT_SLACK_CLIENT_ID" => Some(String::new()),
        _ => None,
    };
    let status = ConnectProviders::from_lookup(all_specs(), lookup);
    assert_eq!(status.reason("google"), None);
    assert_eq!(status.reason("slack"), Some("client_id_not_configured"));
    let rows = serde_json::to_string(&providers_json(all_specs(), &status)).unwrap();
    assert!(!rows.contains("sentinel"), "{rows}");
}
