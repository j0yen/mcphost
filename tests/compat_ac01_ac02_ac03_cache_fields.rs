//! AC1 — Given a server with no tenants, When a client with no
//! `Authorization` header calls `tools/list`, Then the serialized JSON
//! result contains a numeric `ttlMs` and a `cacheScope` of `"private"`,
//! alongside `signup` and the discoverable `host.*` control plane
//! (PRD-mcphost-session-key requirement 1 widened this list from `signup`
//! alone).
//! AC2 — Given the same unauthenticated call, When the raw HTTP response
//! body is parsed as JSON without any client-side model defaults applied,
//! Then both the `ttlMs` and `cacheScope` keys are present in the result
//! object.
//! AC3 — Given a client sending `Authorization: Bearer not-a-real-key`,
//! When it calls `tools/list`, Then the result contains a numeric `ttlMs`
//! and `cacheScope: "private"` and the same anonymous-shaped tool set.

mod common;
use common::{McpClient, TestServer};
use serde_json::Value;

#[tokio::test]
async fn ac1_anonymous_tools_list_has_cache_fields() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed unauthenticated");

    assert!(
        tools["ttlMs"].is_u64(),
        "ttlMs must be a JSON number: {tools}"
    );
    assert_eq!(
        tools["cacheScope"].as_str(),
        Some("private"),
        "cacheScope must be \"private\": {tools}"
    );
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"signup"), "{names:?}");
    assert_eq!(
        names.len(),
        49,
        "signup + host.* (incl. host.quickstart, host.tool_run, and host.bridge_test) + \
         host.tool_call + host.state.* (9 tools, PRD-mcphost-tenant-state) + \
         host.tool_share/host.tool_unshare/host.group.*/host.catalog.* (8 tools, \
         PRD-mcphost-sharing) + host.runs.* (5 tools, PRD-mcphost-runs-and-jobs) + \
         host.trigger.* (9 tools, PRD-mcphost-schedules, PRD-mcphost-inbound-events) + billing.* (3 tools): {names:?}"
    );
}

/// AC2's whole point is to settle a discrepancy in the recorded evidence by
/// looking at the wire bytes, not a client-side model's defaults. So this
/// test bypasses `McpClient` entirely and does the raw HTTP POST itself,
/// parsing the body straight into a `serde_json::Value` -- there is no
/// rmcp type anywhere in this path that could paper over a missing key
/// with a default.
#[tokio::test]
async fn ac2_raw_wire_json_carries_both_keys() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        }))
        .send()
        .await
        .expect("send tools/list");
    assert!(resp.status().is_success(), "status: {}", resp.status());

    let text = resp.text().await.expect("read raw body");
    let body: Value = serde_json::from_str(&text).expect("body is valid JSON");
    let result = body
        .get("result")
        .expect("response has a top-level \"result\" key");

    assert!(
        result
            .as_object()
            .expect("result is an object")
            .contains_key("ttlMs"),
        "raw wire JSON must literally contain the \"ttlMs\" key: {text}"
    );
    assert!(
        result
            .as_object()
            .expect("result is an object")
            .contains_key("cacheScope"),
        "raw wire JSON must literally contain the \"cacheScope\" key: {text}"
    );
}

#[tokio::test]
async fn ac3_invalid_bearer_tools_list_has_cache_fields() {
    let server = TestServer::start().await;
    let client = McpClient::with_bearer(&server.base_url, "not-a-real-key");

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed with an invalid bearer (falls back to anonymous)");

    assert!(
        tools["ttlMs"].is_u64(),
        "ttlMs must be a JSON number: {tools}"
    );
    assert_eq!(
        tools["cacheScope"].as_str(),
        Some("private"),
        "cacheScope must be \"private\": {tools}"
    );
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"signup"), "{names:?}");
    assert_eq!(
        names.len(),
        49,
        "signup + host.* (incl. host.quickstart, host.tool_run, and host.bridge_test) + \
         host.tool_call + host.state.* (9 tools, PRD-mcphost-tenant-state) + \
         host.tool_share/host.tool_unshare/host.group.*/host.catalog.* (8 tools, \
         PRD-mcphost-sharing) + host.runs.* (5 tools, PRD-mcphost-runs-and-jobs) + \
         host.trigger.* (9 tools, PRD-mcphost-schedules, PRD-mcphost-inbound-events) + billing.* (3 tools): {names:?}"
    );
}
