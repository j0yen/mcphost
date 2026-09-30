//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC1 (P0) -- Given `host.quickstart` called with `kind: "webhook"`, When
//! the server responds, Then the spec's `kind` is `http`, `resolved_from`
//! is `webhook`, and `recipe.steps` lists `host.tool_publish`,
//! `host.trigger.set`, `host.trigger.test` in order with `args_example` on
//! each.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_webhook_alias_resolves_to_http_recipe() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "webhook"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&result);

    assert_eq!(structured["kind"], json!("http"));
    assert_eq!(structured["resolved_from"], json!("webhook"));

    let recipe = &structured["recipe"];
    assert_eq!(recipe["name"], json!("webhook-inbox"));
    assert_eq!(recipe["docs"], json!("host.quickstart"));
    let steps = recipe["steps"].as_array().expect("recipe.steps array");
    let tools: Vec<&str> = steps.iter().filter_map(|s| s["tool"].as_str()).collect();
    assert_eq!(
        tools,
        vec!["host.tool_publish", "host.trigger.set", "host.trigger.test"],
        "recipe.steps out of order: {tools:?}"
    );
    for step in steps {
        assert!(
            step["args_example"].is_object(),
            "every recipe step must carry args_example: {step:?}"
        );
    }
}
