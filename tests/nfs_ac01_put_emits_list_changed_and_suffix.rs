//! PRD-mcphost-notes-to-future-self AC1 (P0) -- Given a live keyed connection
//! to a test server, When `host.note.put {text, attach_to: {tool: "cleaner"}}`
//! succeeds, Then the server sent `notifications/tools/list_changed` on that
//! connection and the next `tools/list` describes `cleaner` with the
//! ` — note to self: ` suffix (the falsifier).

use crate::common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::{Value, json};

pub async fn description_of(client: &McpClient, qualified: &str) -> String {
    let listed = client.tools_list().await.expect("tools/list");
    listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == qualified)
        .unwrap_or_else(|| panic!("{qualified} not listed"))["description"]
        .as_str()
        .expect("description")
        .to_string()
}

#[tokio::test]
async fn put_sends_list_changed_and_the_next_tools_list_carries_the_suffix() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "NFS AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let qualified = publish(&client, "cleaner", "echo", json!({"schema": {"type": "object"}})).await;
    let before = description_of(&client, &qualified).await;
    assert!(!before.contains(" — note to self: "), "no note yet: {before}");

    let messages = client
        .tools_call_collect_messages(
            "host.note.put",
            json!({"text": "needs sep=';' for Joe's files", "attach_to": {"tool": "cleaner"}}),
        )
        .await;
    let methods: Vec<&str> = messages.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(methods, ["notifications/tools/list_changed"], "frames: {messages:?}");
    let result = messages.last().expect("final frame");
    assert!(result.get("result").is_some(), "the last frame is the result: {result}");
    let put = extract_structured(&result["result"]);
    assert_eq!(put["visible_in"], "tools/list", "{put}");
    assert_eq!(put["expires_at"], Value::Null);

    let after = description_of(&client, &qualified).await;
    assert_eq!(after, format!("{before} — note to self: needs sep=';' for Joe's files"));

    // Deleting it sends list_changed again and removes the suffix.
    let key_of_note = put["key"].as_str().expect("key").to_string();
    let messages = client.tools_call_collect_messages("host.note.delete", json!({"key": key_of_note})).await;
    let methods: Vec<&str> = messages.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(methods, ["notifications/tools/list_changed"], "frames: {messages:?}");
    assert_eq!(description_of(&client, &qualified).await, before);
}
