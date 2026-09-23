//! PRD-mcphost-python-dependency-policy
//! AC3 (P0) — Given a requirement pinned to a version with a known advisory
//! and a fix, When published with `MCPHOST_ADVISORY_MODE=fail`, Then
//! publish fails naming the advisory and the fixed version.
//!
//! `urllib3==1.26.4` / `GHSA-5phf-pp7p-vc2r` (fixed 1.26.5) is this crate's
//! own embedded advisory-table seed (`deps::embedded_advisories`), not a
//! live feed lookup -- see that function's doc comment.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn known_advisory_fails_publish_when_mode_is_fail() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // SAFETY: this file has exactly one #[tokio::test] fn, so no sibling
    // test in this binary can observe (or race) this process-wide env var
    // -- same rationale `kinds::python`'s own
    // `ensure_uv_discoverable_for_test` doc comment gives for its `PATH`
    // mutation.
    unsafe {
        std::env::set_var("MCPHOST_ADVISORY_MODE", "fail");
    }

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import urllib3\ndef main(args):\n    return {\"ok\": True}\n",
        "requirements": ["urllib3==1.26.4"],
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "vulnerable", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("publish must fail when a known advisory's fix is available");

    unsafe {
        std::env::remove_var("MCPHOST_ADVISORY_MODE");
    }

    assert_eq!(err.error_code.as_deref(), Some("dependency_advisory"), "{err:?}");
    assert!(
        err.message.contains("GHSA-5phf-pp7p-vc2r"),
        "must name the advisory id: {err:?}"
    );
    assert!(
        err.message.contains("urllib3"),
        "must name the package: {err:?}"
    );
    assert!(
        err.message.contains("fix=1.26.5"),
        "must name the fixed version: {err:?}"
    );
}
