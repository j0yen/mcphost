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
    // `sandbox::fork_storm_cap_is_reliable`'s doc comment (PRD-mcphost-call-
    // limits-honest five-whys, commit cfbf672): `RLIMIT_NPROC` has never
    // bound the real (not namespace-mapped) uid 0 at the kernel level, so
    // this AC's cap is a genuine no-op whenever the test binary itself runs
    // as real root (observed on a build lane whose tree lives under
    // `/root/build/...`) -- not a defect in `bwrap_command`'s `prlimit`
    // injection, which the same doc comment traces reliably fails an
    // unprivileged fork storm 62-then-`EAGAIN`. `main.rs`'s `Command::Serve`
    // already refuses to start as root for exactly this reason; skip here
    // rather than fail an AC whose guarantee this execution context cannot
    // hold in the first place.
    // SAFETY: getuid() takes no arguments and cannot fail.
    let real_uid = unsafe { libc::getuid() };
    if !sandbox::fork_storm_cap_is_reliable(real_uid) {
        println!(
            "skipped: fork-storm cap is a kernel-level no-op for real uid 0 \
             (sandbox::fork_storm_cap_is_reliable) -- rerun as an unprivileged user"
        );
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        // A plain `\n\`-continued string strips leading whitespace from
        // each continuation line (that's how Rust joins them), which
        // silently flattens this source's indentation; a raw string with
        // real newlines avoids that trap and reads as actual Python.
        "source": r#"import os
def main(args):
    n = args.get("n", 500)
    children = []
    for _ in range(n):
        pid = os.fork()
        if pid == 0:
            os._exit(0)
        children.append(pid)
    for pid in children:
        os.waitpid(pid, 0)
    return {"forked": len(children)}
"#,
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
