//! PRD-mcphost-auth-error-names-argument
//! AC5 (P0) — Given `get_info` and `host.quickstart`, When read, Then each
//! mentions `tenant_key_missing` once.

mod common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn get_info_instructions_mention_both_codes_once() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let body = client.initialize().await;
    let instructions = body["result"]["instructions"]
        .as_str()
        .expect("initialize result has instructions")
        .to_string();

    assert_eq!(
        instructions.matches("tenant_key_missing").count(),
        1,
        "get_info instructions must mention tenant_key_missing exactly once: {instructions}"
    );
    assert_eq!(
        instructions.matches("tenant_key_invalid").count(),
        1,
        "get_info instructions must mention tenant_key_invalid exactly once: {instructions}"
    );
}

#[tokio::test]
async fn host_quickstart_instructions_mention_both_codes_once() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("host.quickstart is readable before signup");
    let structured = extract_structured(&result);
    let note = structured["steps"][0]["note"]
        .as_str()
        .expect("quickstart's signup step has a note")
        .to_string();

    assert_eq!(
        note.matches("tenant_key_missing").count(),
        1,
        "host.quickstart's note must mention tenant_key_missing exactly once: {note}"
    );
    assert_eq!(
        note.matches("tenant_key_invalid").count(),
        1,
        "host.quickstart's note must mention tenant_key_invalid exactly once: {note}"
    );
}
