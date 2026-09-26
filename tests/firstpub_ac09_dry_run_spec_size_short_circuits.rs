//! PRD-mcphost-first-publish-real-kind
//! AC9 (P0) -- Given a 65 KiB spec with `dry_run: true`, When checked, Then
//! `gates` includes `spec_size` failing with the limit named and no other
//! gate is evaluated after it.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn oversized_spec_dry_run_reports_only_spec_size() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let oversized_description = "x".repeat(65 * 1024);
    let spec = json!({"schema": {"type": "object", "description": oversized_description}});
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "toobig", "kind": "echo", "spec": spec, "dry_run": true}),
        )
        .await
        .expect("dry_run never errors at the RPC level");
    let structured = extract_structured(&result);

    assert_eq!(structured["ok"], json!(false), "{structured}");
    let gates = structured["gates"].as_array().expect("gates array");
    assert_eq!(
        gates.len(),
        1,
        "no other gate must be evaluated after spec_size fails: {gates:?}"
    );
    assert_eq!(gates[0]["gate"], json!("spec_size"));
    assert_eq!(gates[0]["ok"], json!(false));
    let message = gates[0]["message"].as_str().expect("message string");
    assert!(
        message.contains(&mcphost::state::MAX_SPEC_BYTES.to_string()),
        "message must name the limit: {message}"
    );
    let fix = gates[0]["fix"].as_str().expect("fix string");
    assert!(!fix.is_empty(), "fix must be non-empty");

    // dry_run must never write.
    let tools = extract_structured(
        &client
            .tools_call("host.tool_list", json!({}))
            .await
            .expect("tool_list"),
    );
    assert_eq!(
        tools["tools"].as_array().map(Vec::len),
        Some(0),
        "dry_run must publish nothing: {tools}"
    );
}
