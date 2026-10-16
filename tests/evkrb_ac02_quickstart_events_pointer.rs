//! PRD-mcphost-event-kinds-registry-generated-b AC2 — Given an anonymous `host.quickstart`, When it
//! returns, Then `events.kinds == 10` and `events.url == "/events.json"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn evkrb_ac02() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let out = client.tools_call("host.quickstart", json!({})).await.expect("anonymous quickstart");
    let result = extract_structured(&out);
    assert_eq!(result["authenticated"], false, "{result}");
    assert_eq!(result["events"]["kinds"], 10, "{result}");
    assert_eq!(result["events"]["url"], "/events.json", "{result}");
}
