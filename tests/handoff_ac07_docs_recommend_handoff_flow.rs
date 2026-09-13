//! PRD-mcphost-handoff-token
//! AC7 (P0) — Given quickstart, get_info, and llms.txt after build, When
//! read, Then the handoff flow is documented as recommended with the raw
//! path noted compatible.

use crate::common;
use common::{McpClient, TestServer};

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[tokio::test]
async fn get_info_instructions_recommend_handoff_and_note_raw_path_compat() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let body = client.initialize().await;
    let instructions = body["result"]["instructions"]
        .as_str()
        .expect("initialize result has instructions")
        .to_string();

    assert!(
        instructions.contains("handoff: true") && instructions.contains("host.redeem"),
        "get_info instructions must recommend signup(handoff: true) + host.redeem: {instructions}"
    );
    assert!(
        instructions.contains("host.key_rotate"),
        "get_info instructions must mention host.key_rotate: {instructions}"
    );
    assert!(
        instructions.contains("raw-key path") || instructions.contains("fully supported"),
        "get_info instructions must note the raw-key path stays supported: {instructions}"
    );
}

#[tokio::test]
async fn host_quickstart_unauthenticated_note_recommends_handoff() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call("host.quickstart", serde_json::json!({}))
        .await
        .expect("host.quickstart is readable before signup");
    let structured = common::extract_structured(&result);
    let note = structured["steps"][0]["note"]
        .as_str()
        .expect("quickstart's signup step has a note")
        .to_string();
    assert!(
        note.contains("handoff: true") && note.contains("host.redeem"),
        "host.quickstart's pre-signup note must recommend the handoff flow: {note}"
    );
}

#[test]
fn llms_txt_documents_the_handoff_flow_as_recommended() {
    assert!(
        LLMS_TXT.contains("handoff: true") && LLMS_TXT.contains("host.redeem"),
        "www/llms.txt must document signup(handoff: true) + host.redeem"
    );
    assert!(
        LLMS_TXT.contains("host.key_rotate"),
        "www/llms.txt must document host.key_rotate"
    );
}
