//! AC3 — Given a `free` tenant with 5 versions, When a sixth is
//! published, Then version 1 is deleted and versions 2–6 remain.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn sixth_publish_prunes_the_oldest_free_plan_version() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Publisher").await; // defaults to plan "free"
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 1..=6 {
        publish(
            &client,
            "greet",
            "echo",
            json!({"schema": {"type": "object", "properties": {"n": {"const": i}}}}),
        )
        .await;
    }

    let history = client
        .tools_call("host.tool_history", json!({"name": "greet"}))
        .await
        .expect("tool_history");
    let versions: Vec<i64> = extract_structured(&history)["versions"]
        .as_array()
        .expect("versions array")
        .iter()
        .map(|v| v["version"].as_i64().expect("version number"))
        .collect();

    assert_eq!(versions.len(), 5, "free plan keeps 5 versions: {versions:?}");
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, vec![2, 3, 4, 5, 6], "versions 2-6 must remain, 1 pruned: {versions:?}");

    let list = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("tool_list");
    let tools = extract_structured(&list)["tools"].as_array().expect("tools array").clone();
    let greet = tools
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with(".greet"))
        .expect("greet tool listed");
    assert_eq!(greet["current_version"], json!(6));
    assert_eq!(greet["versions"], json!(5));
}
