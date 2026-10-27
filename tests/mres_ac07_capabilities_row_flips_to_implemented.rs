//! AC7 (P0) -- PRD-mcphost-resources-for-tenant-data. Given `capabilities.rs`,
//! When `initialize` is answered, Then `capabilities.resources` is
//! `{subscribe: true}` (PRD-mcphost-subscriptions-listen R11b turned
//! `listChanged` off: no listen path carries resource list changes) and `/capabilities.json` shows
//! `resources.implemented == true`, `probe == "resources"`.

use crate::common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn mres_ac07_initialize_and_capabilities_json_say_resources_is_implemented() {
    let server = TestServer::start().await;
    let init = McpClient::new(&server.base_url).initialize().await;
    assert_eq!(
        init["result"]["capabilities"]["resources"],
        json!({"subscribe": true}),
        "{init}"
    );

    let doc: serde_json::Value = reqwest::get(format!("{}/capabilities.json", server.base_url))
        .await
        .expect("GET /capabilities.json")
        .json()
        .await
        .expect("json");
    let row = &doc["primitives"]["resources"];
    assert_eq!(row["implemented"], true, "{doc}");
    assert_eq!(row["probe"], "resources", "{doc}");
    assert_eq!(row["list_changed"], false, "{doc}");
    assert_eq!(row["subscribe"], true, "{doc}");
    assert_eq!(row["ceiling"], "resources_max", "{doc}");
}

/// Requirement 6: `list_changed` fires after a `host.table.create`, on a
/// connection that had listed resources.
#[tokio::test]
async fn mres_ac07_table_create_sends_resources_list_changed_to_a_listing_connection() {
    use crate::mres_ac04_subscribe_gets_updated_on_append::{SessionClient, serve_sessions};
    let server = TestServer::start().await;
    let (_ns, key) = crate::common::signup(&server.base_url, "mres ac07").await;
    let url = serve_sessions(&server).await;
    let mut peer = SessionClient::open(&url, &key).await;
    let listed = peer.rpc("resources/list", json!({})).await;
    assert!(listed.get("result").is_some(), "{listed}");

    McpClient::with_bearer(&server.base_url, &key)
        .tools_call("host.table.create", json!({"name": "fresh", "columns": {"n": "integer"}}))
        .await
        .expect("create");
    let frames = peer.frames_within(std::time::Duration::from_millis(500)).await;
    let changed = frames.iter().filter(|f| f["method"] == "notifications/resources/list_changed").count();
    assert_eq!(changed, 1, "{frames:?}");
}
