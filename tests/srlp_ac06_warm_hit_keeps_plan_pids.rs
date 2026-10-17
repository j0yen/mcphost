//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC6 (P0) -- Given a warm-pool sandbox for a free tenant, When a second
//! call arrives, Then the sandbox's pids rlimit equals the free
//! `sandbox_pids_max` (a warm hit does not widen limits).
//!
//! The tool reports its own `RLIMIT_NPROC` and its process start time
//! (`/proc/self/stat` field 22). The first (cold) call seeds the pool with a
//! FRESH interpreter, so two consecutive calls reporting the same start time
//! were served by one pooled (warm) interpreter -- and every call, cold or
//! warm, must report the free plan's pids rlimit.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry_with_warm_pool, signup};
use mcphost::plans::PlanCatalog;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn srlp_ac06_warm_sandbox_keeps_the_free_pids_rlimit() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let free_pids = PlanCatalog::default_catalog().get("free").expect("free plan").sandbox_pids_max;
    assert_eq!(free_pids, 16);

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry_with_warm_pool(
        &envs_dir.0,
        std::time::Duration::from_secs(60),
        4,
        8,
    ))
    .await;
    let (ns, key) = signup(&server.base_url, "SRLP AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import resource\ndef main(args):\n    soft, hard = resource.getrlimit(resource.RLIMIT_NPROC)\n    stat = open('/proc/self/stat').read()\n    started = stat.rsplit(') ', 1)[1].split(' ')[19]\n    return {\"soft\": soft, \"hard\": hard, \"started\": started}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "limits", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let mut warm_hit_seen = false;
    let mut previous_start = String::new();
    for round in 1..=4 {
        let result = client
            .tools_call(&format!("{ns}.limits"), json!({}))
            .await
            .unwrap_or_else(|e| panic!("call {round}: {e:?}"));
        let last = extract_structured(&result);
        assert_eq!(last["soft"], json!(free_pids), "round {round}: {last}");
        assert_eq!(last["hard"], json!(free_pids), "round {round}: {last}");
        let started = last["started"].as_str().expect("start time").to_string();
        warm_hit_seen |= started == previous_start;
        previous_start = started;
    }
    assert!(warm_hit_seen, "two consecutive calls must share one pooled interpreter (same process start time)");
}
