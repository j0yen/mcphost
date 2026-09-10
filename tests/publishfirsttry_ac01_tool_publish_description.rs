//! AC1 -- Given an authenticated `tools/list`, When `host.tool_publish` is
//! read, Then its description names every registered kind and the dry-run
//! to try first.
//!
//! Updated by PRD-mcphost-surface-fluidity requirement 6 (P1, AC7): the
//! original design embedded one complete example spec per registered kind
//! directly in this description (asserted here as literal JSON substrings)
//! -- that's the "900 characters across four descriptions" reading tax the
//! newer PRD's audit flagged. The per-kind examples now live in
//! `host.quickstart(kind)` (see `publishfirsttry_ac03_ac04_quickstart.rs`,
//! which still asserts the publish step's spec is kind-real), and this
//! description is measured at <= 600 chars by
//! `tests/surface_ac07_tool_publish_description_length.rs`. This file keeps
//! the surviving half of AC1: every kind is still named, and
//! `host.tool_test` is still the dry run pointed at.

mod common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn description_names_every_kind_and_the_dry_run_to_try_first() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Description Reader").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    let publish = tools
        .iter()
        .find(|t| t["name"] == json!("host.tool_publish"))
        .expect("host.tool_publish is listed");
    let description = publish["description"]
        .as_str()
        .expect("description is a string");

    assert!(
        description.len() <= 600,
        "description must be at most 600 chars (PRD-mcphost-surface-fluidity AC7), got {}: {description}",
        description.len()
    );
    for kind in ["echo", "http", "python"] {
        assert!(
            description.contains(kind),
            "description must mention kind '{kind}': {description}"
        );
    }
    assert!(
        description.contains("host.tool_test"),
        "requirement 5: description must name host.tool_test as the dry run: {description}"
    );
    assert!(
        description.contains("host.quickstart"),
        "per-kind examples now live in host.quickstart(kind); the description must point there: {description}"
    );
}

/// The same description is shown to an unauthenticated caller (requirement
/// 1's whole point: the shape is discoverable from `tools/list` alone,
/// before signup).
#[tokio::test]
async fn description_also_visible_unauthenticated() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let client = common::McpClient::new(&server.base_url);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    let publish = tools
        .iter()
        .find(|t| t["name"] == json!("host.tool_publish"))
        .expect("host.tool_publish is listed to anonymous callers");
    let description = publish["description"]
        .as_str()
        .expect("description is a string");
    assert!(description.len() <= 600);
    assert!(description.contains("python"));
}
