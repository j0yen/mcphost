//! PRD-mcphost-tools-list-alias-truth
//! AC3 — Given a client calls `host.run_snippet`, When dispatched, Then the
//! response is `error: unknown_tool` with `nearest` listing at most 3
//! advertised names including `host.runs.get`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;
use std::collections::HashSet;

async fn nearest_for(client: &McpClient, listed: &HashSet<String>, via_wrapper: bool) {
    let err = if via_wrapper {
        client.tools_call("host.tool.call", json!({"name": "host.run_snippet", "args": {}})).await
    } else {
        client.tools_call("host.run_snippet", json!({})).await
    }
    .expect_err("host.run_snippet is not a tool");

    assert_eq!(err.data["error"], "unknown_tool", "{err:?}");
    assert_eq!(err.data["name"], "host.run_snippet", "{err:?}");
    let nearest: Vec<&str> =
        err.data["nearest"].as_array().expect("nearest array").iter().map(|v| v.as_str().unwrap()).collect();
    assert!(!nearest.is_empty() && nearest.len() <= 3, "at most 3 nearest: {nearest:?}");
    assert!(nearest.contains(&"host.runs.get"), "host.runs.get must be nearest: {nearest:?}");
    for n in &nearest {
        assert!(listed.contains(*n), "{n} is not advertised by tools/list");
    }
}

#[tokio::test]
async fn unknown_host_name_answers_with_the_nearest_advertised_tools() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AliasTruth AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let listed: HashSet<String> =
        listed["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();

    nearest_for(&client, &listed, false).await;
    nearest_for(&client, &listed, true).await;
}
