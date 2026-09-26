//! PRD-mcphost-first-publish-real-kind
//! AC3 (P0) -- Given a python spec that references a missing secret,
//! collides on an env var, and requests egress on a free plan, When
//! `host.tool_publish {dry_run: true}` runs, Then `ok == false` and `gates`
//! lists all three with `fix` text, and no tool row exists afterwards.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn dry_run_reports_every_failing_gate_at_once() {
    // Reaches `Kind::validate_async`'s sandboxed AST check before the gate
    // checks run, same as `plainenv_ac04`'s own doc comment explains.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // A pre-existing secret named MODE so publishing env: {"MODE": ...}
    // collides with it.
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
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gated", "kind": "python", "spec": spec, "dry_run": true}),
        )
        .await
        .expect("dry_run never errors at the RPC level");
    let structured = extract_structured(&result);

    assert_eq!(structured["ok"], json!(false), "{structured}");
    let gates = structured["gates"].as_array().expect("gates array");

    let failing: Vec<&str> = gates
        .iter()
        .filter(|g| g["ok"] == json!(false))
        .map(|g| g["gate"].as_str().expect("gate name"))
        .collect();
    for expected in ["secrets", "env", "network"] {
        assert!(
            failing.contains(&expected),
            "expected {expected} among the failing gates, got {failing:?}: {structured}"
        );
    }
    for gate in gates.iter().filter(|g| g["ok"] == json!(false)) {
        assert!(
            gate["fix"].as_str().is_some_and(|s| !s.is_empty()),
            "every failing gate must carry non-empty fix text: {gate}"
        );
    }

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
