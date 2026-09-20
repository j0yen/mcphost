//! PRD-mcphost-tenant-data-export
//! AC5 (P1) -- Given the manifest, When each entry is validated against the
//! `host.tool_publish` input schema, Then all validate.

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
async fn manifest_entries_validate_against_tool_publish_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Export AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["alpha", "beta"] {
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
            .tools_call("host.export", json!({}))
            .await
            .expect("host.export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");
    let manifest = finished["result"]["manifest"]
        .as_array()
        .expect("manifest array")
        .clone();
    assert_eq!(manifest.len(), 2, "manifest: {manifest:?}");

    let schema = mcphost::handler::tool_publish_input_schema();
    let validator = jsonschema::validator_for(&schema).expect("compile host.tool_publish schema");
    for entry in &manifest {
        assert!(
            validator.is_valid(entry),
            "manifest entry does not validate against host.tool_publish's own input schema: {entry:?}"
        );
    }
}
