//! AC2 (PRD-mcphost-quickstart-by-client) — anonymous `host.quickstart`
//! with `clientInfo.name: "cursor"` returns the cursor `client` block.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::install_links::surface_for;
use serde_json::json;

#[tokio::test]
async fn cursor_client_info_gets_the_deeplink_block() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("cursor", "1.0");
    let qs = extract_structured(&client.tools_call("host.quickstart", json!({})).await.expect("quickstart"));
    let block = &qs["client"];
    assert_eq!(block["detected"], "cursor", "{qs}");
    assert_eq!(block["from"], "clientInfo");
    assert_eq!(block["install"]["kind"], "deeplink");
    let expected = surface_for(&server.base_url, "cursor").expect("cursor surface").artefact;
    assert_eq!(block["snippet"], json!(expected));
    assert!(block["caveats"].as_array().is_some_and(|c| !c.is_empty()));
}
