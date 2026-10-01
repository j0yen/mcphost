//! AC5 (P0) — Given `tools/list`, When fetched, Then every `host.*` and
//! `billing.*` tool's `inputSchema.additionalProperties == false`.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn every_host_and_billing_tool_declares_additional_properties_false() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC5").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let tools_list = client.tools_list().await.expect("tools/list");
    let tools = tools_list["tools"].as_array().expect("tools array");

    let mut checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default();
        if !(name.starts_with("host.") || name.starts_with("billing.")) {
            continue;
        }
        checked += 1;
        if tool["inputSchema"]["additionalProperties"] != json!(false) {
            offenders.push(name.to_string());
        }
    }
    assert!(offenders.is_empty(), "tools missing additionalProperties: false: {offenders:?}");
    assert!(
        checked > 50,
        "sanity: this host registers well over 50 host.*/billing.* tools, got {checked}"
    );
}
