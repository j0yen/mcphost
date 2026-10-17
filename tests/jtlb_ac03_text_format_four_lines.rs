//! PRD-mcphost-job-timeline-tool-b AC3 (P1) -- Given `format: "text"` on a
//! 3-step run, When rendered, Then the output is 4 lines, each step line
//! contains the tool name and `duration_ms`, and the last line starts with
//! `totals`.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn jtlb_ac03() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "JTLB3 Tenant").await;
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
            {"tool": "step2", "args": {"n": "$.input.n"}},
            {"tool": "step3", "args": {"n": "$.input.n"}}
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({"n": 5})).await.expect("chain call");
    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let run_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();

    let out = extract_structured(
        &client
            .tools_call("host.job.timeline", json!({"run_id": run_id, "format": "text"}))
            .await
            .expect("timeline text"),
    );
    let text = out["text"].as_str().expect("text field");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    for (line, tool) in lines[..3].iter().zip(["step1", "step2", "step3"]) {
        assert!(line.contains(tool) && line.contains("duration_ms"), "{line}");
    }
    assert!(lines[3].starts_with("totals"), "{text}");
}
