//! PRD-mcphost-tool-infer AC13 (P0) — Given the same source published
//! twice (as two distinctly-named tools), When both inferred schemas are
//! serialized, Then they are byte-identical including property order.
//!
//! `kinds::infer`'s own unit tests (`deterministic_property_order`) already
//! prove this at the pure-function level; this test proves it holds
//! through the real publish -> `tools/list` path too.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn two_tools_from_identical_source_get_byte_identical_inferred_schemas() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC13 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let source = "def main(args):\n    return args[\"zeta\"], args[\"alpha\"], args.get(\"middle\", \"m\")\n";
    for name in ["copy_a", "copy_b"] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "python", "spec": {"source": source}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name} ok: {} {}", e.code, e.message));
    }

    let listed = client.tools_list().await.expect("tools/list ok");
    let tools = listed["tools"].as_array().expect("tools array");
    let schema_a = tools
        .iter()
        .find(|t| t["name"] == format!("{ns}.copy_a"))
        .expect("copy_a listed")["inputSchema"]
        .clone();
    let schema_b = tools
        .iter()
        .find(|t| t["name"] == format!("{ns}.copy_b"))
        .expect("copy_b listed")["inputSchema"]
        .clone();

    assert_eq!(
        serde_json::to_string(&schema_a).unwrap(),
        serde_json::to_string(&schema_b).unwrap(),
        "identical source must infer byte-identical schemas, including property order"
    );
}
