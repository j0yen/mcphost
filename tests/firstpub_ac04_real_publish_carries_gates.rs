//! PRD-mcphost-first-publish-real-kind
//! AC4 (P0) -- Given the same spec as AC3 without `dry_run`, When published,
//! Then the error carries the same three-entry `gates` array.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn a_real_publish_failing_multiple_gates_reports_every_one() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.secret_set", json!({"name": "MODE", "value": "v"}))
        .await
        .expect("secret_set ok");

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "secrets": ["missing_secret"],
        "env": {"MODE": "fast"},
        "network": "egress",
    });

    // First, confirm the dry_run shape (same spec as AC3) so this test is
    // self-contained about what "the same gates array" means.
    let dry_run_result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gated", "kind": "python", "spec": spec.clone(), "dry_run": true}),
        )
        .await
        .expect("dry_run ok");
    let dry_run_gates = extract_structured(&dry_run_result)["gates"].clone();

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gated", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("a spec failing multiple gates must still fail the real publish");

    let gates = err.data.get("gates").cloned().expect("data.gates present");
    assert_eq!(
        gates, dry_run_gates,
        "a real publish's gates must match dry_run's own report for the same spec"
    );

    let failing: Vec<&str> = gates
        .as_array()
        .expect("gates array")
        .iter()
        .filter(|g| g["ok"] == json!(false))
        .map(|g| g["gate"].as_str().expect("gate name"))
        .collect();
    assert_eq!(
        failing.len(),
        3,
        "exactly three gates fail for this spec: {failing:?}"
    );
    for expected in ["secrets", "env", "network"] {
        assert!(
            failing.contains(&expected),
            "expected {expected} among the failing gates, got {failing:?}"
        );
    }

    // No tool row was written.
    let tools = extract_structured(
        &client
            .tools_call("host.tool_list", json!({}))
            .await
            .expect("tool_list"),
    );
    assert_eq!(
        tools["tools"].as_array().map(Vec::len),
        Some(0),
        "a failed publish must write nothing: {tools}"
    );
}
