//! PRD-mcphost-notes-to-future-self AC2 (P0) -- Given a 300-char note
//! attached to a tool, When `tools/list` renders, Then the suffix text is
//! exactly 120 chars ending in `…`, cut at a char boundary, and
//! `host.note.list` still returns the full text.

use crate::common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn long_note_is_cut_to_120_chars_in_tools_list_and_whole_in_note_list() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "NFS AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let qualified = publish(&client, "cleaner", "echo", json!({"schema": {"type": "object"}})).await;

    // Multi-byte chars, so a byte cut would split one.
    let text: String = "é".repeat(300);
    client
        .tools_call("host.note.put", json!({"text": text, "attach_to": {"tool": "cleaner"}}))
        .await
        .expect("put");

    let listed = client.tools_list().await.expect("tools/list");
    let description = listed["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == qualified.as_str())
        .expect("cleaner listed")["description"]
        .as_str()
        .expect("description")
        .to_string();
    let (_, suffix) = description.split_once(" — note to self: ").expect("suffix present");
    assert_eq!(suffix.chars().count(), 120, "suffix: {suffix}");
    assert!(suffix.ends_with('…'));
    assert_eq!(suffix, format!("{}…", "é".repeat(119)));

    let notes = extract_structured(&client.tools_call("host.note.list", json!({})).await.expect("list"));
    assert_eq!(notes["notes"][0]["text"], text.as_str());
    assert_eq!(notes["notes"][0]["attach_to"], json!({"tool": "cleaner"}));
}
