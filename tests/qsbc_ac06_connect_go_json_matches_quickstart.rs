//! AC6 (PRD-mcphost-quickstart-by-client) — `/connect/go/cursor?format=json`
//! equals quickstart's `client` block for cursor, byte for byte except `from`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

#[tokio::test]
async fn connect_go_json_equals_quickstart_client_block_except_from() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("cursor", "1.0");
    let qs = extract_structured(&client.tools_call("host.quickstart", json!({})).await.expect("quickstart"));
    let mut from_quickstart = qs["client"].clone();

    let resp = reqwest::get(format!("{}/connect/go/cursor?format=json", server.base_url)).await.expect("get");
    assert_eq!(resp.status(), 200);
    let mut from_route: Value = resp.json().await.expect("json body");

    assert_eq!(from_quickstart["detected"], "cursor");
    assert_eq!(from_route["detected"], "cursor");
    from_quickstart["from"] = Value::Null;
    from_route["from"] = Value::Null;
    assert_eq!(from_route.to_string(), from_quickstart.to_string());
}
