//! AC4 (PRD-mcphost-quickstart-by-client) — an unrecognised caller gets a
//! `client` block of nulls, empty caveats, and the install doc URL.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn unknown_caller_gets_null_block_and_doc_url() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let qs = extract_structured(
        &client
            .tools_call_with_header("host.quickstart", json!({}), ("user-agent", "curl/8.0"))
            .await
            .expect("quickstart"),
    );
    let block = &qs["client"];
    for key in ["detected", "from", "install", "snippet", "next"] {
        assert!(block[key].is_null(), "{key} must be null: {qs}");
    }
    assert_eq!(block["caveats"], json!([]));
    assert_eq!(block["all"], json!(mcphost::install_links::install_doc_url()));
}
