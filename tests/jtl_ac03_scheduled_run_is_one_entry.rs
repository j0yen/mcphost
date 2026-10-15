//! PRD-mcphost-job-timeline-tool AC3 (P0) -- Given a scheduled run of a tool,
//! When the timeline is read, Then `steps` has one entry whose `tool` is the
//! run's tool, `run.trigger` is `schedule`, and `totals.steps` is 1.
//!
//! The tool is `echo`-kind: the timeline reads the run row alone, so the
//! tool's kind does not change what it renders (a python tool needs the
//! sandbox, which this non-sandbox suite does not assume).

use crate::common;
use common::{extract_structured, signup_and_make_pro};
use serde_json::json;

#[tokio::test]
async fn jtl_ac03() {
    let server = common::TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "JTL3 Tenant", "cus_jtl3").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client.tools_call("host.trigger.set", json!({"tool": "pinger", "schedule": "0 0 1 1 *"})).await.expect("set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();
    let fired = extract_structured(
        &client.tools_call("host.trigger.fire", json!({"id": trigger_id})).await.expect("fire"),
    );
    let run_id = fired["run_id"].as_str().expect("run_id").to_string();

    let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));
    let timeline = extract_structured(
        &client.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect("timeline"),
    );

    assert_eq!(timeline["run"]["trigger"], json!("schedule"), "{timeline}");
    let steps = timeline["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{timeline}");
    assert_eq!(steps[0]["tool"], got["tool"], "{timeline}");
    assert_eq!(steps[0]["run_id"], json!(run_id), "{timeline}");
    assert_eq!(steps[0]["step"], json!(1), "{timeline}");
    assert_eq!(timeline["totals"]["steps"], json!(1), "{timeline}");
}
