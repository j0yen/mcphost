//! AC6 (P1) -- PRD-mcphost-server-capabilities-from-registry. Anonymous and
//! keyed `host.quickstart` carry `capabilities.implemented` (exactly the
//! implemented row names) and `capabilities.unsupported` (the rest), both in
//! registry order.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::capabilities::CAPABILITIES;
use serde_json::{Value, json};

fn names(v: &Value) -> Vec<String> {
    v.as_array().expect("array").iter().map(|n| n.as_str().expect("string").to_string()).collect()
}

fn assert_capabilities(result: &Value, who: &str) {
    let caps = &result["capabilities"];
    let implemented: Vec<String> =
        CAPABILITIES.iter().filter(|c| c.implemented).map(|c| c.name.to_string()).collect();
    let unsupported: Vec<String> =
        CAPABILITIES.iter().filter(|c| !c.implemented).map(|c| c.name.to_string()).collect();
    assert_eq!(names(&caps["implemented"]), implemented, "{who}: capabilities.implemented in {result}");
    assert_eq!(names(&caps["unsupported"]), unsupported, "{who}: capabilities.unsupported in {result}");
}

#[tokio::test]
async fn anonymous_and_keyed_quickstart_list_the_registry_rows() {
    let server = TestServer::start().await;

    let anonymous = McpClient::new(&server.base_url);
    let out = anonymous.tools_call("host.quickstart", json!({})).await.expect("anonymous quickstart");
    let result = extract_structured(&out);
    assert_eq!(result["authenticated"], false, "{result}");
    assert_capabilities(&result, "anonymous");

    let (_ns, key) = signup(&server.base_url, "creg ac06").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let out = keyed.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("keyed quickstart");
    let result = extract_structured(&out);
    assert_capabilities(&result, "keyed");
}
