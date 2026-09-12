//! PRD-mcphost-tool-infer AC4 (P0) — Given a `python` spec that supplies
//! its own `args_schema`, When it is published, Then that schema is used
//! unchanged and no inference is performed.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn explicit_schema_is_used_verbatim_not_merged_with_inference() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Source reads `args["city"]` (which inference would make required),
    // but the author supplies an explicit schema naming a *different*
    // property (`zip`) instead.
    let explicit_schema = json!({
        "type": "object",
        "properties": {"zip": {"type": "string"}},
        "required": ["zip"],
    });
    let spec = json!({
        "source": "def main(args):\n    return args.get(\"city\", args.get(\"zip\"))\n",
        "args_schema": explicit_schema,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "explicit", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == format!("{ns}.explicit"))
        .expect("published tool listed")
        .clone();

    // Exactly the author's schema, not a merge with what inference would
    // have derived from `args["city"]`.
    assert_eq!(tool["inputSchema"], explicit_schema);
    assert!(
        tool["inputSchema"]["properties"].get("city").is_none(),
        "an inferred property must never leak into an explicit schema"
    );
}
