//! AC5 (PRD-mcphost-quickstart-by-client) — `clientInfo.name: "chatgpt"`
//! gets the OAuth consent caveat and `host.whoami` as the next call.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn chatgpt_gets_oauth_caveat_and_whoami_next() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("chatgpt", "1.0");
    let qs = extract_structured(&client.tools_call("host.quickstart", json!({})).await.expect("quickstart"));
    let block = &qs["client"];
    assert_eq!(block["detected"], "chatgpt", "{qs}");
    let caveats = block["caveats"].as_array().expect("caveats");
    assert!(
        caveats.iter().any(|c| c.as_str().is_some_and(|c| c.contains("OAuth") && c.contains("Create a new workspace"))),
        "{caveats:?}"
    );
    assert_eq!(block["next"]["tool"], "host.whoami");
}
