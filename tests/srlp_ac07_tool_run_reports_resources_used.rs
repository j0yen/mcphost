//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC7 (P1) -- Given `host.tool_run` on an unpublished spec, When it
//! completes, Then `resources_used` is present with the same four keys.
//!
//! `host.tool_run` and `host.tool_test` are the debug/dry-run verbs an agent
//! uses to measure a tool; both are exercised on a tool that has no `calls`
//! history (nothing but the publish), and both must report the same object
//! a real call does.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

fn assert_four_keys(result: &Value) {
    let out = result;
    let used = &result["_meta"]["resources_used"];
    for key in ["cpu_ms", "peak_rss_mb", "wall_ms"] {
        assert!(used[key].is_i64(), "resources_used.{key} must be an integer: {out}");
    }
    assert!(used.get("pids_max_seen").is_some(), "resources_used.pids_max_seen key present: {out}");
}

#[tokio::test]
async fn srlp_ac07_tool_run_and_tool_test_report_resources_used() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "SRLP AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "measured", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let run = client.tools_call("host.tool_run", json!({"name": "measured", "args": {}})).await.expect("tool_run");
    assert_four_keys(&run);
    assert!(
        extract_structured(&run).get("resources_used").is_none(),
        "usage rides in _meta, not the envelope body: {run}"
    );

    let test = client.tools_call("host.tool_test", json!({"name": "measured", "args": {}})).await.expect("tool_test");
    assert_four_keys(&test);
    assert!(
        extract_structured(&test).get("resources_used").is_none(),
        "usage rides in _meta, not the envelope body: {test}"
    );
}
