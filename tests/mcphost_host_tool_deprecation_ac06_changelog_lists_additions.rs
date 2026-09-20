//! PRD-mcphost-host-tool-deprecation AC6 — Given prod after ship, When
//! `host.changelog {since: "0.57.0"}` is called against it, Then it lists
//! the additions since 0.57.0 including `host.changelog` itself.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn changelog_since_0_57_0_lists_additions_including_itself() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.changelog", json!({"since": "0.57.0"}))
        .await
        .expect("host.changelog must succeed");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["since"], "0.57.0");
    let additions = structured["additions"]
        .as_array()
        .expect("additions must be an array");
    assert!(
        !additions.is_empty(),
        "at least one addition (host.changelog itself) must be listed: {structured}"
    );
    let names: Vec<&str> = additions
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(
        names.contains(&"host.changelog"),
        "host.changelog must list itself as an addition since 0.57.0: {names:?}"
    );

    assert!(structured["deprecations"].is_array());
    assert!(structured["removals"].is_array());
}

#[tokio::test]
async fn changelog_with_no_since_still_lists_host_changelog() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant No Since").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.changelog", json!({}))
        .await
        .expect("host.changelog must succeed with no since argument");
    let structured = common::extract_structured(&result);
    let names: Vec<&str> = structured["additions"]
        .as_array()
        .expect("additions array")
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(names.contains(&"host.changelog"), "{names:?}");
}
