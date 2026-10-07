//! PRD-mcphost-tool-call-host-verb-forward
//! AC2 — Given the same tenant, When `host.tool_call` is sent
//! `host_trigger_set`, `host.trigger_set`, and `trigger_set` with the same
//! args, Then all three forward identically and `host.trigger.list` shows
//! exactly one trigger per distinct `name`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn every_underscore_dot_spelling_forwards_to_the_same_trigger() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish pinger");

    // `*/5 * * * *`: a 300s interval, exactly the free plan's own
    // `schedule_min_interval_s` floor -- a plain `signup()` tenant (not
    // `signup_and_make_pro`) stays on `free`.
    let args = json!({"tool": "pinger", "kind": "schedule", "schedule": "*/5 * * * *"});

    let mut results: Vec<Value> = Vec::new();
    for spelling in ["host_trigger_set", "host.trigger_set", "trigger_set"] {
        let call = client
            .tools_call("host.tool_call", json!({"name": spelling, "args": args.clone()}))
            .await
            .unwrap_or_else(|e| panic!("host.tool_call name=\"{spelling}\" must forward: {} {}", e.code, e.message));
        let structured = extract_structured(&call);
        assert_eq!(
            structured["forwarded_to"], "host.trigger.set",
            "spelling {spelling} must forward to host.trigger.set: {structured}"
        );
        assert_eq!(
            structured["client_tool"], "host_trigger_set",
            "spelling {spelling} must carry client_tool: {structured}"
        );
        results.push(structured);
    }

    // Same tool+kind, no explicit `name` in any of the three calls -- all
    // three resolve to the same default trigger identity
    // (`triggers::set`'s own `<kind>:<tool>`), so only the first call
    // creates it and the other two are idempotent re-sets of the same row.
    assert_eq!(results[0]["created"], json!(true), "the first call must create: {:?}", results[0]);
    assert_eq!(results[1]["created"], json!(false), "the second call must update, not create: {:?}", results[1]);
    assert_eq!(results[2]["created"], json!(false), "the third call must update, not create: {:?}", results[2]);

    let list = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("host.trigger.list"),
    );
    let triggers = list["triggers"].as_array().expect("triggers array");
    let names: Vec<&str> = triggers.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names,
        vec!["schedule:pinger"],
        "exactly one trigger must exist for the one distinct name: {names:?}"
    );
}
