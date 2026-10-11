//! PRD-mcphost-webhook-inbox-one-call
//! AC5 -- Given a keyed `host.quickstart(kind="inbox")`, When it returns,
//! Then `recipe.name == "webhook-inbox"`, `recipe.steps` has one entry
//! calling `host.webhook_inbox.create`, `recipes_nearby` contains
//! `agent-inbox`, and the anonymous quickstart note contains "tenant_key on
//! every call" and not "connection is bound".

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn keyed_inbox_quickstart_is_one_step_with_agent_inbox_nearby() {
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs.0)).await;
    let (_ns, key) = signup(&server.base_url, "whin ac5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for word in ["inbox", "webhook"] {
        let out = extract_structured(
            &client.tools_call("host.quickstart", json!({"kind": word})).await.expect("quickstart"),
        );
        assert_eq!(out["recipe"]["name"], json!("webhook-inbox"), "{word}: {out}");
        let steps = out["recipe"]["steps"].as_array().unwrap_or_else(|| panic!("{word}: steps: {out}"));
        assert_eq!(steps.len(), 1, "{word}: {out}");
        assert_eq!(steps[0]["tool"], json!("host.webhook_inbox.create"), "{word}: {out}");
    }

    let inbox = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "inbox"})).await.expect("quickstart inbox"),
    );
    let nearby = inbox["recipes_nearby"].as_array().unwrap_or_else(|| panic!("recipes_nearby: {inbox}"));
    assert!(nearby.contains(&json!("agent-inbox")), "{inbox}");
}

#[tokio::test]
async fn anonymous_quickstart_note_states_the_stateless_rule() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);
    let out = extract_structured(&anon.tools_call("host.quickstart", json!({})).await.expect("anonymous quickstart"));
    let notes: Vec<String> = out["steps"]
        .as_array()
        .unwrap_or_else(|| panic!("steps: {out}"))
        .iter()
        .filter_map(|s| s["note"].as_str().map(str::to_string))
        .collect();
    let all = notes.join("\n");
    assert!(all.contains("tenant_key on every call"), "{all}");
    assert!(!all.contains("connection is bound"), "{all}");
    assert!(!out.to_string().contains("connection is bound"), "{out}");
}
