//! PRD-mcphost-tool-infer AC10 (P0, shared with AC12) — Given a source
//! importing a module absent from the import-to-distribution map, When it
//! is published with no `requirements`, Then publish fails with a
//! structured error naming that module and no tool is created.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn unmapped_import_fails_publish_naming_the_module() {
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
    let (ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import definitely_not_a_known_package\n\ndef main(args):\n    return {}\n",
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "unmapped", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("an unmapped import must fail publish, not guess a distribution");

    // AC12: a stable, machine-readable code.
    assert_eq!(err.error_code.as_deref(), Some("requirement_not_inferable"));
    // AC10: the error names the exact module.
    assert!(
        err.message.contains("definitely_not_a_known_package"),
        "error must name the unmapped module, got: {}",
        err.message
    );
    assert_eq!(
        err.data["module"], "definitely_not_a_known_package",
        "structured data must name the module too"
    );

    // No tool was created.
    let listed = client.tools_list().await.expect("tools/list ok");
    let names: Vec<String> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    assert!(!names.contains(&format!("{ns}.unmapped")));
}
