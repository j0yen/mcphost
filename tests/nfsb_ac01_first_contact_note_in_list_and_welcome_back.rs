//! PRD-mcphost-notes-to-future-self-b AC1 (P0) -- Given a first-contact note
//! written by `signup {remember}`, When `host.note.list` runs, Then it appears
//! as `{text, attach_to: null}` and `welcome_back.notes` counts it exactly once.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

#[tokio::test]
async fn nfsb_ac01() {
    let server = TestServer::start().await;
    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    session1
        .tools_call("signup", json!({"name": "NFSB AC1", "remember": "cleaning Joe's CSVs"}))
        .await
        .expect("signup with remember");
    session1.tools_call("host.whoami", json!({})).await.expect("whoami");

    let list = extract_structured(&session1.tools_call("host.note.list", json!({})).await.expect("note.list"));
    let notes = list["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 1, "{list}");
    assert_eq!(notes[0]["text"], json!("cleaning Joe's CSVs"), "{list}");
    assert_eq!(notes[0]["attach_to"], Value::Null, "{list}");

    let rotated = extract_structured(&session1.tools_call("host.key_rotate", json!({})).await.expect("rotate"));
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = extract_structured(&session2.tools_call("host.whoami", json!({})).await.expect("first call"));
    assert_eq!(first_call["welcome_back"]["notes"], json!(1), "{first_call}");
    assert_eq!(first_call["welcome_back"]["last_note"], json!("cleaning Joe's CSVs"), "{first_call}");
}
