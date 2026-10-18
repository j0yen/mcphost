//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC4 (P0) -- Given a free tenant's tool that forks 20 processes, When it
//! is called, Then the 17th fork fails inside the sandbox and the call's
//! `resources_used.pids_max_seen` is <= 16.
//!
//! The sandbox's own interpreter is one of the free plan's 16 pids, so the
//! fork that fails is the 16th at the latest: the assertions below hold the
//! PRD's "<= 16" bound rather than one exact count.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const FREE_PIDS: i64 = 16;

#[tokio::test]
async fn srlp_ac04_twenty_forks_hit_the_free_pids_ceiling() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // SAFETY: getuid() takes no arguments and cannot fail.
    if !sandbox::fork_storm_cap_is_reliable(unsafe { libc::getuid() }) {
        println!("skipped: RLIMIT_NPROC does not bind real uid 0");
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "SRLP AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": r#"import os, signal, time
def main(args):
    kids = []
    failed_at = None
    for i in range(20):
        try:
            pid = os.fork()
        except OSError:
            failed_at = i + 1
            break
        if pid == 0:
            time.sleep(5)
            os._exit(0)
        kids.append(pid)
    time.sleep(0.3)
    for pid in kids:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    return {"forked": len(kids), "failed_at": failed_at}
"#,
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "forker", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let result = client
        .tools_call(&format!("{ns}.forker"), json!({}))
        .await
        .expect("the tool catches its own fork failure and returns");
    let out = extract_structured(&result);
    let failed_at = out["failed_at"].as_i64().unwrap_or_else(|| panic!("no fork failed: {out}"));
    assert!(failed_at <= FREE_PIDS + 1, "a fork must fail by the 17th, failed_at={failed_at}: {out}");
    assert!(out["forked"].as_i64().unwrap_or(99) <= FREE_PIDS, "{out}");
    let seen = result["_meta"]["resources_used"]["pids_max_seen"]
        .as_i64()
        .unwrap_or_else(|| panic!("pids_max_seen must be sampled: {out}"));
    assert!((2..=FREE_PIDS).contains(&seen), "pids_max_seen={seen}: {out}");
}
