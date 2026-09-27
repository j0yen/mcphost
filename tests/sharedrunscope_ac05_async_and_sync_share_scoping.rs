//! PRD-mcphost-shared-call-run-scope
//! AC5 — Given B calls the shared tool async and sync once each, When B
//! calls `host.runs.list`, Then both runs appear with the same `tool`
//! string and scoping -- proving the sync path (this PRD) and the async
//! path (`runs::enqueue_shared`, pre-existing) now agree.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn async_and_sync_shared_calls_share_the_same_tool_string_and_scoping() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "lookup", "visibility": "public", "description": "lookup"}),
        )
        .await
        .expect("O shares lookup publicly");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_o}.lookup");

    client_b
        .tools_call("host.tool_call", json!({"name": qualified, "args": {"msg": "sync"}}))
        .await
        .expect("B calls O's shared tool synchronously");

    let enqueue_result = client_b
        .tools_call(
            "host.tool_call",
            json!({"name": qualified, "args": {"msg": "async"}, "async": true}),
        )
        .await
        .expect("B enqueues an async call on O's shared tool");
    let run_id = extract_structured(&enqueue_result)["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();
    client_b
        .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 10}))
        .await
        .expect("B waits for its async run to finish");

    let runs = extract_structured(
        &client_b
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("B lists its own runs"),
    );
    let list = runs["runs"].as_array().expect("runs array");
    assert_eq!(list.len(), 2, "both the sync and async run must appear for B: {runs}");
    for run in list {
        assert_eq!(run["tool"], json!(qualified), "both runs must share the same qualified tool string: {run}");
    }
    let triggers: std::collections::BTreeSet<&str> =
        list.iter().map(|r| r["trigger"].as_str().expect("trigger")).collect();
    assert_eq!(
        triggers,
        std::collections::BTreeSet::from(["call", "job"]),
        "one sync (call) and one async (job) run must both be present: {runs}"
    );

    let owner_runs = extract_structured(
        &client_o
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("O lists its own runs"),
    );
    assert!(
        owner_runs["runs"].as_array().expect("runs array").is_empty(),
        "the owner must see neither of B's runs: {owner_runs}"
    );
}
