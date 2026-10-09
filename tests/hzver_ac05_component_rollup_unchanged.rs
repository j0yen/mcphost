//! PRD-mcphost-healthz-version-field AC5 — Given `/status.json` with
//! `component`+`days` params, When fetched, Then the per-component rollup is
//! unchanged and carries no `version` (only the whole-feed body does).

use crate::common;
use common::TestServer;
use mcphost::state::{now_unix, rfc3339_from_unix};

#[tokio::test]
async fn component_rollup_has_no_build_keys_but_whole_feed_does() {
    let server = TestServer::start().await;
    let day = rfc3339_from_unix(now_unix())[..10].to_string();
    server
        .state
        .db
        .upsert_status_daily("mcp".to_string(), day, 1400, 1440, 20)
        .await
        .expect("upsert_status_daily");

    let rollup: serde_json::Value =
        reqwest::get(format!("{}/status.json?component=mcp&days=1", server.base_url))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let mut keys: Vec<&str> = rollup.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["component", "days"], "rollup: {rollup}");
    assert!(rollup.get("version").is_none() && rollup.get("git_sha").is_none());

    let feed: serde_json::Value = reqwest::get(format!("{}/status.json", server.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(feed.get("version").is_some() && feed.get("git_sha").is_some());
}
