//! PRD-mcphost-shared-call-run-scope
//! AC1 — Given O shares `lookup` public and B calls `host.tool_call
//! {name: "<O>.lookup"}` synchronously, When B calls `host.runs.list`, Then
//! one run with `tool == "<O>.lookup"`, `trigger == "call"`, `status ==
//! "done"` (this codebase's terminal-success value, same as every other
//! `runs` row -- see `runs_ac05_sync_call_writes_run_row.rs`) returns.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn caller_sees_the_shared_sync_call_in_its_own_runs_list() {
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
        .tools_call("host.tool_call", json!({"name": qualified, "args": {"msg": "hi"}}))
        .await
        .expect("B calls O's shared tool synchronously");

    let runs = extract_structured(
        &client_b
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("B lists its own runs"),
    );
    let list = runs["runs"].as_array().expect("runs array");
    assert_eq!(list.len(), 1, "exactly one run for B: {runs}");
    let run = &list[0];
    assert_eq!(run["tool"], json!(qualified));
    assert_eq!(run["trigger"], json!("call"));
    assert_eq!(run["status"], json!("done"));
}
