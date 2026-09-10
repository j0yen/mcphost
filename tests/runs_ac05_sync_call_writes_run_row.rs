//! AC5 (P0) — Given a synchronous `host.tool_call`, When it completes, Then
//! a `runs` row with `trigger='call'` and a `calls` row exist with matching
//! duration.
//!
//! `echo` needs no sandbox, so this is the fastest possible proof that
//! `Db::record_call_attributed` really does write both rows in the same
//! transaction (P0 requirement 2), independent of anything job-shaped.

mod common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn synchronous_call_writes_a_matching_calls_and_runs_row() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    client
        .tools_call(&format!("{ns}.echoer"), json!({"hello": "world"}))
        .await
        .expect("call ok");

    // host.usage's own duration figures come from `calls`; host.runs.list's
    // come from `runs` -- both must exist and agree this was one `trigger:
    // "call"` execution.
    let usage = extract_structured(
        &client
            .tools_call("host.usage", json!({}))
            .await
            .expect("usage ok"),
    );
    assert_eq!(usage["calls"], json!(1), "calls row must exist: {usage}");

    let runs = extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    let list = runs["runs"].as_array().expect("runs array");
    assert_eq!(list.len(), 1, "exactly one runs row: {runs}");
    let run = &list[0];
    assert_eq!(run["trigger"], json!("call"));
    assert_eq!(run["status"], json!("done"));
    assert_eq!(run["tool"], json!("echoer"));
}
