//! PRD-mcphost-stdlib-pseudo-modules AC2 (P0) — Given source importing
//! `__main__`, When published, Then no PyPI requirement is inferred for
//! it.
//!
//! `__main__` is the other always-present pseudo-module the PRD names
//! alongside `__future__`: it is the interpreter's own entry-point module,
//! never a PyPI distribution. Same sandboxed `host.spec_test` -> publish
//! path as AC1's test, proving `__main__` alone (no `from __future__
//! import`) also resolves to an empty `requirements` list rather than
//! `requirement_not_inferable`.

use crate::common;
use common::{
    McpClient, TempDataDir, TestServer, extract_structured, poll_spec_test_until_ready,
    python_kind_registry, signup,
};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn dunder_main_import_infers_no_pypi_requirement() {
    // Requirement 8/9: real sandboxed execution -- see AC1's test in
    // tests/stdlibpseudo_ac1_future_import_empty_requirements.rs for the
    // full rationale on this skip guard.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let data_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Stdlib Pseudo AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let source = "import __main__\n\n\
                  def main(args: dict) -> dict:\n\
                  \x20   return {\"ok\": True}\n";

    let result = poll_spec_test_until_ready(
        &client,
        json!({
            "kind": "python",
            "spec": {"source": source},
            "invocations": [{}],
        }),
        Duration::from_secs(30),
    )
    .await;
    let structured = extract_structured(&result);
    assert_eq!(
        structured["requirements"],
        json!([]),
        "importing __main__ must infer no PyPI requirement, got: {structured:?}"
    );
    assert_eq!(
        structured["invocations"][0]["ok"],
        json!(true),
        "spec_test invocation should succeed: {structured:?}"
    );

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dunder_main_tool", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish importing __main__ must succeed: {} {}", e.code, e.message));
}
