//! PRD-mcphost-shared-call-run-scope
//! AC6 — Given raw `tools/call name="<O>.lookup"` from B, When ledgers are
//! read, Then the outcome matches AC1 and AC2 (both entry points share the
//! path) -- raw dispatch's `Some((ns, local))` arm and `host.tool_call`'s
//! own qualified-name arm both resolve through `call_shared_tool` ->
//! `call_published_tool`, so this is the same
//! `record_call_attributed_with_end_user` call site under a different
//! entry point.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn raw_tools_call_scopes_the_run_to_the_caller_like_host_tool_call_does() {
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

    // Raw `tools/call` (not `host.tool_call`): the qualified name is the
    // JSON-RPC method's own `name`, args are the call's own `arguments`.
    let call = client_b
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("B calls O's shared tool via raw tools/call");
    let result = extract_structured(&call);
    assert_eq!(result["msg"], json!("hi"));

    // AC1's outcome: B sees exactly one run, scoped and named like the
    // host.tool_call path.
    let b_runs = extract_structured(
        &client_b
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("B lists its own runs"),
    );
    let b_list = b_runs["runs"].as_array().expect("runs array");
    assert_eq!(b_list.len(), 1, "exactly one run for B: {b_runs}");
    let run = &b_list[0];
    assert_eq!(run["tool"], json!(qualified));
    assert_eq!(run["trigger"], json!("call"));
    assert_eq!(run["status"], json!("done"));
    let run_id = run["run_id"].as_str().expect("run_id").to_string();

    // AC2's outcome: O sees none of it, and can't resolve B's run id.
    let owner_runs = extract_structured(
        &client_o
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("O lists its own runs"),
    );
    assert!(
        owner_runs["runs"].as_array().expect("runs array").is_empty(),
        "owner must see none of the caller's raw-dispatch runs: {owner_runs}"
    );
    let owner_get_err = client_o
        .tools_call("host.runs.get", json!({"run_id": run_id}))
        .await
        .expect_err("the owner must never resolve the caller's own run id");
    assert_eq!(owner_get_err.error_code.as_deref(), Some("run_not_found"));
}
