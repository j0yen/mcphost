//! PRD-mcphost-stdlib-pseudo-modules AC1 (P0) — Given a python-kind spec
//! whose source contains `from __future__ import annotations` and only
//! stdlib imports, When it is published, Then publish succeeds with an
//! empty inferred requirements list.
//!
//! `__future__` is a compiler-directive pseudo-module: always importable,
//! never installable from PyPI. Before this PRD, `infer-data/python-
//! stdlib.json` had no dunder/pseudo-module entries, so
//! `infer_python_requirements` (src/kinds/infer.rs) treated `__future__`
//! as an unresolvable third-party import and publish failed with
//! `requirement_not_inferable` even though idiomatic modern Python source
//! (`from __future__ import annotations`) never needs a PyPI package for
//! it. This test proves the real `host.spec_test` -> publish path now
//! reports an empty `requirements` list for such source, through the same
//! sandboxed execution path a real call uses (not just the pure-function
//! unit tests in `src/kinds/infer.rs`).

use crate::common;
use common::{
    McpClient, TempDataDir, TestServer, extract_structured, poll_spec_test_until_ready,
    python_kind_registry, signup,
};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn future_import_and_stdlib_only_publishes_with_empty_requirements() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as
    // tests/infer_ac10_unmapped_import_fails.rs and
    // tests/infer_ac13_deterministic_schema.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let data_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Stdlib Pseudo AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let source = "from __future__ import annotations\n\
                  import json\n\
                  import os\n\n\
                  def main(args: dict) -> dict:\n\
                  \x20   return {\"ok\": True}\n";

    // Proves inference through host.spec_test's real response shape first
    // (it echoes the inferred `requirements` directly).
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
        "from __future__ import annotations plus stdlib-only imports must \
         infer an empty requirements list, got: {structured:?}"
    );
    assert_eq!(
        structured["invocations"][0]["ok"],
        json!(true),
        "spec_test invocation should succeed: {structured:?}"
    );

    // And proves the same source publishes cleanly end-to-end (this is
    // exactly the shape of the failure this PRD is grounded in: AC6 live
    // publish of the fleet adapter's agent.py was rejected here).
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "future_import_tool", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish with __future__ import must succeed: {} {}", e.code, e.message));
}
