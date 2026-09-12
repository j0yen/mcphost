//! AC4 -- Given the `initialize` response, When its instructions text is
//! read, Then it states how kind is chosen and how to request one
//! explicitly.

use crate::common;
use common::TestServer;
use mcphost::kinds::KindRegistry;

#[tokio::test]
async fn initialize_instructions_explain_kind_choice() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let client = common::McpClient::new(&server.base_url);

    let body = client.initialize().await;
    let instructions = body["result"]["instructions"]
        .as_str()
        .expect("initialize result has instructions");

    // How kind is chosen: the spec's own shape decides (source -> python;
    // upstream/method/url -> http).
    assert!(
        instructions.contains("source") && instructions.contains("python"),
        "instructions must explain that an inline `source` field implies python: {instructions}"
    );
    assert!(
        instructions.contains("http"),
        "instructions must explain the http side of kind resolution: {instructions}"
    );
    // How to request one explicitly, and what happens on disagreement.
    assert!(
        instructions.contains("kind_mismatch"),
        "instructions must name the kind_mismatch error a caller can expect on disagreement: \
         {instructions}"
    );
}
