//! PRD-mcphost-tool-versions
//! AC3 (P0) — Given a `free` tenant with 5 versions, When a sixth is
//! published, Then version 1 is deleted and versions 2–6 remain.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn spec_requiring(field: &str) -> serde_json::Value {
    json!({"schema": {"type": "object", "properties": {field: {"type": "string"}}, "required": [field]}})
}

#[tokio::test]
async fn sixth_publish_prunes_version_one_keeps_two_through_six() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Free plan's versions_max is 5 -- publish six times.
    for i in 1..=6 {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": "capped", "kind": "echo", "spec": spec_requiring(&format!("f{i}"))}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish v{i} should succeed: {} {}", e.code, e.message));
    }

    let history = client
        .tools_call("host.tool_history", json!({"name": "capped"}))
        .await
        .expect("tool_history should succeed");
    let versions = extract_structured(&history)["versions"].clone();
    let versions = versions.as_array().expect("versions array");
    let numbers: Vec<i64> = versions
        .iter()
        .map(|v| v["version"].as_i64().unwrap())
        .collect();

    assert_eq!(
        numbers.len(),
        5,
        "exactly 5 versions should survive on the free plan: {numbers:?}"
    );
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        vec![2, 3, 4, 5, 6],
        "version 1 must be pruned, versions 2-6 must remain: {numbers:?}"
    );
}
