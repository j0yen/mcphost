//! AC5 (P0) -- PRD-mcphost-prompts-from-published-tools. Publishing then
//! removing a prompt-kind tool puts exactly two
//! `notifications/prompts/list_changed` frames on the caller's connection
//! (one per call, ahead of that call's own result); publishing a python tool
//! puts none.

use crate::common;
use crate::promptkind;

use common::{McpClient, TestServer, signup};
use promptkind::{PromptKind, classify_spec};
use serde_json::{Value, json};

const METHOD: &str = "notifications/prompts/list_changed";

fn count(messages: &[Value]) -> usize {
    messages.iter().filter(|m| m["method"] == METHOD).count()
}

#[tokio::test]
async fn mprm_ac05_list_changed_on_prompt_publish_and_remove() {
    let kind = PromptKind::new();
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(kind.registry_with_python(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "mprm ac05").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);

    let published = tenant
        .tools_call_collect_messages("host.tool_publish", json!({"name": "classify_ticket", "kind": "prompt", "spec": classify_spec()}))
        .await;
    assert!(published.last().is_some_and(|m| m.get("result").is_some()), "{published:?}");
    let removed = tenant.tools_call_collect_messages("host.tool_remove", json!({"name": "classify_ticket"})).await;
    assert!(removed.last().is_some_and(|m| m.get("result").is_some()), "{removed:?}");
    assert_eq!(count(&published) + count(&removed), 2, "published: {published:?} removed: {removed:?}");
    for batch in [&published, &removed] {
        let n = batch.iter().position(|m| m["method"] == METHOD).expect("notification present");
        assert_eq!(n, batch.len() - 2, "notification precedes the result: {batch:?}");
    }

    let python = tenant
        .tools_call_collect_messages(
            "host.tool_publish",
            json!({"name": "adder", "kind": "python", "spec": {"source": "def main(args):\n    return {\"ok\": True}\n"}}),
        )
        .await;
    assert!(python.last().is_some_and(|m| m.get("result").is_some()), "{python:?}");
    assert_eq!(count(&python), 0, "a python publish sends no prompts/list_changed: {python:?}");
    let removed_python = tenant.tools_call_collect_messages("host.tool_remove", json!({"name": "adder"})).await;
    assert_eq!(count(&removed_python), 0, "{removed_python:?}");
}
