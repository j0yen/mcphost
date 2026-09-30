//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC7 (P1) -- Given `host.quickstart` with no kind, When the server
//! responds, Then the response lists `kinds`, `aliases` and `recipes`
//! arrays.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_with_no_kind_lists_kinds_aliases_and_recipes() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("quickstart with no kind");
    let structured = extract_structured(&result);

    // No kind given defaults to the python starter recipe (unchanged, AC1
    // of PRD-mcphost-first-publish-real-kind) -- this AC is purely
    // additive on top of that.
    assert_eq!(structured["kind"], json!("python"));

    let kinds = structured["kinds"].as_array().expect("kinds array");
    assert!(kinds.iter().any(|k| k == "http"), "kinds: {kinds:?}");
    assert!(kinds.iter().any(|k| k == "python"), "kinds: {kinds:?}");

    let aliases = structured["aliases"].as_array().expect("aliases array");
    assert!(aliases.iter().any(|a| a == "webhook"), "aliases: {aliases:?}");
    assert!(aliases.iter().any(|a| a == "cron"), "aliases: {aliases:?}");

    let recipes = structured["recipes"].as_array().expect("recipes array");
    assert!(
        recipes.iter().any(|r| r == "webhook-inbox"),
        "recipes: {recipes:?}"
    );
    assert!(recipes.iter().any(|r| r == "schedules"), "recipes: {recipes:?}");
}
