//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC5 (P0) -- Given any successful python call, When the result is
//! returned, Then it carries `resources_used` with integer `cpu_ms`,
//! `peak_rss_mb`, `wall_ms` and `wall_ms >= cpu_ms / 4`.
//!
//! The usage rides in the CallToolResult `_meta.resources_used`, never inside
//! the tool's own returned object (a pass-through tool's result must stay the
//! host verb's JSON byte for byte).
//!
//! Two calls: the first is cold, the second is served from the warm pool
//! (when the pool promoted the first), so both usage-reporting paths are
//! covered.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

fn assert_resources_used(result: &Value) {
    let out = result;
    let used = &result["_meta"]["resources_used"];
    let cpu = used["cpu_ms"].as_i64().unwrap_or_else(|| panic!("integer cpu_ms: {out}"));
    let rss = used["peak_rss_mb"].as_i64().unwrap_or_else(|| panic!("integer peak_rss_mb: {out}"));
    let wall = used["wall_ms"].as_i64().unwrap_or_else(|| panic!("integer wall_ms: {out}"));
    assert!(cpu >= 0 && rss >= 0, "{out}");
    assert!(wall >= cpu / 4, "wall_ms {wall} must be >= cpu_ms {cpu} / 4: {out}");
    assert!(used.get("pids_max_seen").is_some(), "pids_max_seen key present (null until sampled): {out}");
}

#[tokio::test]
async fn srlp_ac05_cold_and_warm_python_results_carry_resources_used() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "SRLP AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"n\": sum(i * i for i in range(20000))}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "sumsq", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");
    for round in 0..2 {
        let result = client
            .tools_call(&format!("{ns}.sumsq"), json!({}))
            .await
            .unwrap_or_else(|e| panic!("call {round} must succeed: {e:?}"));
        let out = extract_structured(&result);
        assert!(out.get("n").is_some() || out.get("payload").is_some(), "tool result still present: {out}");
        assert!(out.get("resources_used").is_none(), "usage must not ride inside the tool result: {out}");
        assert_resources_used(&result);
    }
}
