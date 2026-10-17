//! PRD-mcphost-job-timeline-tool-b AC2 (P0) -- Given `TIMELINE_FIELDS`, When
//! the contract test renders a timeline, Then every JSON field is in the table
//! and every table row is present; and Given the same run read by
//! `host.runs.get`, Then `started_unix`, `duration_ms`, `status`, and
//! `error_class` agree per child.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn jtlb_ac02() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "JTLB2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    for name in ["step1", "step2", "step3"] {
        publish(&client, name, "echo", json!({"schema": {"type": "object"}})).await;
    }
    let chain = publish(
        &client,
        "pipeline",
        "chain",
        json!({"steps": [
            {"tool": "step1", "args": {"n": "$.input.n"}},
            {"tool": "step2", "args": {"m": "$.prev.result.absent"}, "on_error": "continue"},
            {"tool": "step3", "args": {"k": "$.input.n"}}
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({"n": 5})).await.expect("chain call");
    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let run_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();

    let timeline =
        extract_structured(&client.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect("timeline"));
    let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));

    let steps = timeline["steps"].as_array().expect("steps");
    let children = got["children"].as_array().expect("children");
    assert_eq!(steps.len(), 3, "{timeline}");
    assert_eq!(steps.len(), children.len(), "{timeline} vs {got}");

    let mut table: Vec<&str> = mcphost::timeline::TIMELINE_FIELDS.iter().map(|(f, _)| *f).collect();
    table.sort_unstable();
    for step in steps {
        let mut keys: Vec<&str> = step.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, table, "timeline entry fields drifted from TIMELINE_FIELDS: {step}");
    }

    for (step, child) in steps.iter().zip(children) {
        assert_eq!(step["run_id"], child["run_id"], "{step} vs {child}");
        for field in ["started_unix", "duration_ms", "error_class"] {
            assert_eq!(step[field], child[field], "{field}: {step} vs {child}");
        }
        // The timeline folds failed|error|timeout into `error`; any other status reads the same.
        let expected = match child["status"].as_str().unwrap() {
            "failed" | "error" | "timeout" => "error",
            other => other,
        };
        assert_eq!(step["status"], json!(expected), "status: {step} vs {child}");
    }
}
