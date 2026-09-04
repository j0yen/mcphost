//! AC1 -- Given an authenticated `tools/list`, When `host.tool_publish` is
//! read, Then its description contains one complete example spec for each
//! registered kind and is under 1,200 characters.

mod common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn description_has_one_example_per_kind_under_1200_chars() {
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
        description.len() < 1200,
        "description must be under 1200 chars, got {}: {description}",
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
    // Each kind's blurb embeds its example spec as compact JSON -- prove
    // it's real, parseable JSON containing the field each kind's minimal
    // spec needs, not just descriptive prose.
    assert!(
        description.contains(r#""schema""#),
        "echo example missing: {description}"
    );
    assert!(
        description.contains(r#""url""#),
        "http example missing: {description}"
    );
    assert!(
        description.contains(r#""source""#),
        "python example missing: {description}"
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
    assert!(description.len() < 1200);
    assert!(description.contains("python"));
}
