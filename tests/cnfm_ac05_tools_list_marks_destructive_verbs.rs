//! PRD-mcphost-destructive-verb-confirmation-token
//! AC5 -- Given `tools/list` for a keyed tenant, When read, Then every
//! registry verb of a gated kind carries `annotations.destructiveHint ==
//! true` and a description ending in the confirm sentence, every non-gated
//! verb carries neither, and `docs/host-tools.md` contains the sentence once
//! per gated verb.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::confirm::{CONFIRM_SENTENCE, gated, gated_name};
use mcphost::write_verbs::WRITE_VERBS;
use serde_json::Value;
use std::collections::BTreeMap;

#[tokio::test]
async fn gated_verbs_are_marked_destructive_and_say_how_to_confirm() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cnfm AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let tools: BTreeMap<String, Value> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| (t["name"].as_str().expect("tool name").to_string(), t.clone()))
        .collect();

    let gated_verbs: Vec<&str> = WRITE_VERBS.iter().filter(|v| gated(v)).map(|v| v.name).collect();
    assert!(gated_verbs.len() >= 15, "the registry gates its delete/drop/revoke/remove verbs: {gated_verbs:?}");
    for name in &gated_verbs {
        let tool = tools.get(*name).unwrap_or_else(|| panic!("{name} is not in tools/list"));
        assert_eq!(tool["annotations"]["destructiveHint"], Value::Bool(true), "{name}: {tool}");
        assert_eq!(tool["annotations"]["idempotentHint"], Value::Bool(false), "{name}: {tool}");
        let description = tool["description"].as_str().unwrap_or_default();
        assert!(description.ends_with(CONFIRM_SENTENCE), "{name} description must end with the confirm sentence: {description}");
    }

    // Every other dotted tool carries no confirm sentence; its
    // `destructiveHint` is only the PRD-mcphost-tool-idempotent-declaration
    // one (`!idempotent`, from `HOST_TOOL_HINTS`), never the confirm gate's.
    for (name, tool) in tools.iter().filter(|(n, _)| n.contains('.') && !gated_name(n)) {
        let idempotent = tool["annotations"]["idempotentHint"].as_bool();
        assert_eq!(tool["annotations"]["destructiveHint"].as_bool(), idempotent.map(|i| !i), "{name}: {tool}");
        let description = tool["description"].as_str().unwrap_or_default();
        assert!(!description.contains(CONFIRM_SENTENCE), "{name} is not gated: {description}");
    }
}

#[test]
fn host_tools_doc_carries_the_sentence_once_per_gated_verb() {
    let doc = std::fs::read_to_string(format!("{}/docs/host-tools.md", env!("CARGO_MANIFEST_DIR")))
        .expect("docs/host-tools.md");
    let gated_count = WRITE_VERBS.iter().filter(|v| gated(v)).count();
    assert_eq!(doc.matches(CONFIRM_SENTENCE).count(), gated_count, "one sentence per gated verb");
    for verb in WRITE_VERBS.iter().filter(|v| gated(v)) {
        let line = format!("- `{}`: {CONFIRM_SENTENCE}", verb.name);
        assert_eq!(doc.matches(&line).count(), 1, "{} documented exactly once", verb.name);
    }
}
