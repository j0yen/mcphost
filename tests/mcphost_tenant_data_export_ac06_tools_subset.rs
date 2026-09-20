//! PRD-mcphost-tenant-data-export
//! AC6 (P1) -- Given `tools: ["a"]`, When exported, Then only tool `a` is
//! in the archive.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

async fn wait_for_done(client: &McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn tools_filter_exports_only_the_named_subset() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Export AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["tool_a", "tool_b"] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name} failed: {} {}", e.code, e.message));
    }

    let enqueue = extract_structured(
        &client
            .tools_call("host.export", json!({"tools": ["tool_a"]}))
            .await
            .expect("host.export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");

    let manifest = finished["result"]["manifest"]
        .as_array()
        .expect("manifest array");
    let names: Vec<&str> = manifest
        .iter()
        .map(|m| m["name"].as_str().expect("manifest entry name"))
        .collect();
    assert_eq!(names, vec!["tool_a"], "manifest: {manifest:?}");
}
