//! contract: reads registry tool_aliases::TOOL_ALIASES
//! PRD-mcphost-host-changelog-since AC3 — Given `since: "0.0.0"`, When
//! `host.changelog` runs, Then `renamed` has exactly `TOOL_ALIASES.len()`
//! entries and each `from`/`to` pair matches the table.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::tool_aliases::TOOL_ALIASES;
use serde_json::json;

#[tokio::test]
async fn renamed_has_one_entry_per_alias_matching_the_table() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CHG AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let out = extract_structured(
        &client.tools_call("host.changelog", json!({"since": "0.0.0"})).await.expect("changelog"),
    );
    let renamed = out["renamed"].as_array().expect("renamed array");
    assert_eq!(renamed.len(), TOOL_ALIASES.len(), "{renamed:?}");
    for a in TOOL_ALIASES {
        let hit = renamed
            .iter()
            .find(|r| r["from"] == a.alias)
            .unwrap_or_else(|| panic!("alias {} missing from renamed", a.alias));
        assert_eq!(hit["to"], a.canonical, "{hit}");
        assert!(hit["since"].is_string() && hit["sunset"].is_string(), "{hit}");
    }
}
