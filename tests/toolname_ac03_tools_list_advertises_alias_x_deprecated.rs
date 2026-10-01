//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC3 (P0) -- Given `tools/list`, When fetched, Then every canonical name
//! is present, every alias is present with `_meta["x-deprecated"].replaced_by`
//! equal to its canonical and a `sunset` date 90 days after landing, and
//! the alias's `inputSchema` equals the canonical's.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::tool_aliases::{ALIASES, ALIAS_SUNSET_DATE};
use std::collections::HashMap;

#[tokio::test]
async fn every_alias_carries_x_deprecated_and_matches_its_canonical_schema() {
    let server = TestServer::start().await;
    // Authenticated, not anonymous: host.spec.test (and so its alias
    // host.spec_test) is the one host.* descriptor gated to authenticated
    // callers only (PRD-mcphost-tool-test AC9) -- every other alias in
    // ALIASES is visible either way, so the authenticated list is the
    // superset this test needs to see all 20 at once.
    let (_tenant, key) = common::signup(&server.base_url, "ac03-caller").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tools = client.tools_list().await.expect("tools/list");
    let entries = tools["tools"].as_array().expect("tools array");

    let by_name: HashMap<&str, &serde_json::Value> = entries
        .iter()
        .map(|t| (t["name"].as_str().expect("name"), t))
        .collect();

    assert_eq!(ALIASES.len(), 20, "this PRD's own landing count -- update alongside docs/tools.md if it ever changes");

    for (alias, canonical) in ALIASES {
        let canonical_tool = by_name
            .get(canonical)
            .unwrap_or_else(|| panic!("canonical {canonical} missing from tools/list"));
        let alias_tool = by_name
            .get(alias)
            .unwrap_or_else(|| panic!("alias {alias} missing from tools/list"));

        let dep = alias_tool
            .get("_meta")
            .and_then(|m| m.get("x-deprecated"))
            .unwrap_or_else(|| panic!("{alias} must carry _meta.x-deprecated"));
        assert_eq!(dep["replaced_by"], *canonical, "{alias}'s x-deprecated.replaced_by");
        assert_eq!(dep["sunset"], ALIAS_SUNSET_DATE, "{alias}'s x-deprecated.sunset");

        assert_eq!(
            alias_tool["inputSchema"], canonical_tool["inputSchema"],
            "{alias}'s inputSchema must equal {canonical}'s"
        );
        // The canonical itself must never carry the alias's deprecation
        // note -- only the alias does.
        assert!(
            canonical_tool.get("_meta").and_then(|m| m.get("x-deprecated")).is_none(),
            "{canonical} must not itself be marked x-deprecated"
        );
    }
}
