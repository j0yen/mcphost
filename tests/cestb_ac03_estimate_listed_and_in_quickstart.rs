//! PRD-mcphost-call-cost-estimate-b
//! AC3 -- Given `host.quickstart` on a keyed tenant, When `try_before_call`
//! is read, Then it contains a `host.estimate` row and tools/list contains
//! `host.estimate` with `_meta.alias_of: host.tool_call`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn quickstart_names_host_estimate_and_tools_list_carries_it_as_an_alias() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cest Surface").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"),
    );
    let rows = quickstart["try_before_call"].as_array().expect("try_before_call is an array");
    let row = rows
        .iter()
        .find(|r| r["call"] == json!("host.estimate"))
        .unwrap_or_else(|| panic!("try_before_call has no host.estimate row: {rows:?}"));
    assert_eq!(row["note"], json!("free; same verdict as the real call"), "{row}");
    assert!(row["arguments"]["name"].is_string(), "{row}");

    let listed = client.tools_list().await.expect("tools/list");
    let estimate: &Value = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!("host.estimate"))
        .expect("tools/list contains host.estimate for a keyed tenant");
    assert_eq!(estimate["_meta"]["alias_of"], json!("host.tool_call"), "{estimate}");

    // The anonymous listing stays free of it (like host.spec_test).
    let anon = McpClient::new(&server.base_url).tools_list().await.expect("anonymous tools/list");
    assert!(
        anon["tools"].as_array().expect("tools").iter().all(|t| t["name"] != json!("host.estimate")),
        "host.estimate is listed for keyed tenants only"
    );
}
