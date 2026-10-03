//! PRD-mcphost-trigger-set-idempotent
//! AC7 (P1) — Given the quickstart `webhook-inbox` recipe, When read, Then
//! its `set` step shows `name` and states that repeating the call is safe.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn webhook_inbox_recipes_set_step_shows_name_and_states_idempotence() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Trigidem AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "webhook"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&result);
    assert_eq!(structured["kind"], json!("http"));

    let steps = structured["recipe"]["steps"].as_array().expect("recipe.steps array");
    let set_step = steps
        .iter()
        .find(|s| s["tool"] == json!("host.trigger.set"))
        .expect("recipe has a host.trigger.set step");

    assert!(
        set_step["args_example"].get("name").and_then(|v| v.as_str()).is_some(),
        "the set step's args_example must show name: {set_step:?}"
    );

    let note = set_step["note"]
        .as_str()
        .unwrap_or_else(|| panic!("the set step must carry a note: {set_step:?}"));
    let lower = note.to_ascii_lowercase();
    assert!(
        lower.contains("safe") && (lower.contains("repeat") || lower.contains("again") || lower.contains("re-run")),
        "the note must state that repeating the call is safe: {note:?}"
    );
}
