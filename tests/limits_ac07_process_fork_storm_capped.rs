//! AC7 (PRD-mcphost-call-limits-honest) — Given a python tool that forks
//! 500 processes, When called, Then it fails with `tool_process_limit` and
//! the host's process count returns to baseline within 5 s.
//!
//! `sandbox::pre_exec_setup` now sets `RLIMIT_NPROC` (default 64, from
//! `kinds::python`'s `MAX_PROCESSES`) before the isolation wrapper is
//! exec'd; a fork past that count fails with `EAGAIN`, which the runner
//! protocol (`PY_RUNNER_SCRIPT`) catches and reports as the structured
//! `process_limit` envelope kind, mapped here to `tool_process_limit`.
//!
//! The process-count check is a coarse system-wide `/proc` count, not a
//! per-cgroup exact accounting -- this build box runs other work
//! concurrently, so the assertion tolerates ordinary system churn while
//! still catching a real leak of hundreds of leftover processes.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

fn proc_count() -> usize {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
                })
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn fork_storm_fails_with_tool_process_limit_and_processes_are_cleaned_up() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\n\
def main(args):\n\
    n = args.get(\"n\", 500)\n\
    children = []\n\
    for _ in range(n):\n\
        pid = os.fork()\n\
        if pid == 0:\n\
            os._exit(0)\n\
        children.append(pid)\n\
    for pid in children:\n\
        os.waitpid(pid, 0)\n\
    return {\"forked\": len(children)}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 30,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "forkstorm", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.forkstorm");

    let baseline = proc_count();

    let err = client
        .tools_call(&qualified, json!({"n": 500}))
        .await
        .expect_err("a 500-way fork storm must be refused, not silently allowed");
    assert_eq!(err.error_code.as_deref(), Some("tool_process_limit"));

    // Give the kernel/namespace teardown a moment, then confirm the
    // process table is back near baseline (generous tolerance -- this is a
    // shared build box, not an isolated measurement rig).
    tokio::time::sleep(Duration::from_secs(5)).await;
    let after = proc_count();
    let delta = after.abs_diff(baseline);
    assert!(
        delta <= 25,
        "process count must return near baseline within 5s: baseline={baseline} after={after} \
         (delta={delta})"
    );
}
