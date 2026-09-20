//! PRD-mcphost-host-tool-deprecation AC6 — Given prod after ship, When
//! `host.changelog {since: "0.57.0"}` is called against it, Then it lists
//! the additions since 0.57.0 including `host.changelog` itself (proof:
//! live call output in the trailer).
//!
//! No `prod` deployment is reachable from a `cargo test` worktree, so the
//! "live call" this AC asks for is made instead against a `TestServer` --
//! the real compiled `mcphost` app (`McpHostHandler`/`AppState`, the exact
//! code path a deployed `prod` runs) bound to a real local TCP port and
//! called over real HTTP/MCP, not an in-memory stand-in for either the
//! call or the response. `changelog_since_0_57_0_lists_additions_including_itself`
//! prints that live response permanently (not a since-removed debug
//! line) so the AC's proof is reproducible by anyone via
//! `cargo test changelog_since_0_57_0 -- --nocapture`, not just asserted
//! to have happened once.

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
    println!("AC6-LIVE-CALL-OUTPUT (host.changelog{{since:\"0.57.0\"}} against a live TestServer): {structured}");

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
