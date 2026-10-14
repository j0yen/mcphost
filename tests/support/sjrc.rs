//! Shared helpers for the PRD-mcphost-scheduled-job-receipts tests: a probe
//! tool whose runs can be made to fail, firing a trigger and waiting for its
//! receipt. Pulled in per-file with `#[path = "support/sjrc.rs"] mod sjrc;`.

use crate::common::{McpClient, extract_structured};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Publishes `gone_dep` (echo) and `probe`, a chain over it; removing
/// `gone_dep` later makes every `probe` run end `error`.
pub async fn publish_probe(client: &McpClient) {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gone_dep", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish gone_dep");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "probe", "kind": "chain", "spec": {"steps": [{"tool": "gone_dep", "args": {}}]}}),
        )
        .await
        .expect("publish probe");
}

/// Fires `trigger_id` manually and waits until its receipt count reaches `n`.
pub async fn fire_until_receipts(client: &McpClient, trigger_id: &str, n: usize) -> Value {
    client
        .tools_call("host.trigger.fire", json!({"id": trigger_id}))
        .await
        .expect("trigger.fire");
    wait_receipts(client, trigger_id, n).await
}

pub async fn wait_receipts(client: &McpClient, trigger_id: &str, n: usize) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.schedule_receipts", json!({"trigger_id": trigger_id}))
                .await
                .expect("schedule_receipts"),
        );
        if got["summary"]["runs"].as_u64() == Some(n as u64) {
            return got;
        }
        assert!(Instant::now() < deadline, "receipt {n} never appeared: {got:?}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}


/// Publishes (or republishes) the echo dependency `probe` chains over.
pub async fn restore_dep(client: &McpClient) {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gone_dep", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("republish gone_dep");
}

/// Removes the dependency, so every `probe` run ends `error`.
pub async fn break_dep(client: &McpClient) {
    client
        .tools_call("host.tool_remove", json!({"name": "gone_dep"}))
        .await
        .expect("remove gone_dep");
}

/// Every message in the tenant's inbox sent by `@host`, oldest first.
pub async fn host_messages(client: &McpClient) -> Vec<Value> {
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    inbox["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|m| m["from_address"] == json!("@host"))
        .cloned()
        .collect()
}

/// Lets a just-written receipt's follow-on inbox write settle before a
/// "nothing was sent" assertion.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}
