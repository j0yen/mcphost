//! PRD-mcphost-surface-fluidity AC8 (P1, requirement 7) — Given `get_info`,
//! When read, Then its instructions mention `try_before_call`, so a client
//! that reads instructions but not `host.quickstart` still finds the
//! pointer to which dry-run tool fits its case.

mod common;
use common::{McpClient, TestServer};

#[tokio::test]
async fn get_info_instructions_mention_try_before_call() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let body = client.initialize().await;
    let instructions = body["result"]["instructions"]
        .as_str()
        .expect("initialize result has instructions");

    assert!(
        instructions.contains("try_before_call"),
        "get_info instructions must mention try_before_call: {instructions}"
    );
}
