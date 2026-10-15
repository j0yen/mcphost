//! PRD-mcphost-host-changelog-since-b AC1 — Given a tenant with no
//! `last_seen_version`, When `host.changelog {}` runs, Then `since: null`,
//! `releases` has the 5 newest entries, and `added` is empty.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::api_contract::{CHANGELOG_MD, parse_changelog};
use serde_json::json;

#[tokio::test]
async fn chgb_ac01_fresh_tenant_gets_since_null_five_releases_and_no_added() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CHGB AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let out = extract_structured(&client.tools_call("host.changelog", json!({})).await.expect("changelog"));
    assert!(out["since"].is_null(), "{out}");
    let versions: Vec<&str> =
        out["releases"].as_array().unwrap().iter().map(|r| r["version"].as_str().unwrap()).collect();
    let newest: Vec<String> = parse_changelog(CHANGELOG_MD)
        .unwrap()
        .iter()
        .filter(|r| r.version <= mcphost::api_contract::Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
        .take(5)
        .map(|r| r.version.to_string())
        .collect();
    assert_eq!(versions.len(), 5);
    assert_eq!(versions, newest);
    assert!(out["added"].as_array().unwrap().is_empty(), "{}", out["added"]);
}
