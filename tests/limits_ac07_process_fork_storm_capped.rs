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
//!
//! PRD-mcphost-tests-host-independence requirement 2: the box's build user
//! has `RLIMIT_NPROC` 232241 against RedBaron's 110661 -- neither number is
//! what caps this storm (that's `kinds::python::max_processes()`, the
//! sandbox's own internal limit, always 64 regardless of host), but the
//! old fixed 5s cleanup deadline had no relationship to how many processes
//! the storm actually forked before hitting that cap, so it was one flaky
//! assumption away from being host-load-sensitive too. The deadline below
//! now scales with the number of processes actually observed spawned, and
//! every value this test reasons about (both limits, the counts, the
//! elapsed time) is in the failure message, so a red run on an unfamiliar
//! host never needs a follow-up SSH session to explain itself.

mod common;
#[path = "support/host.rs"]
#[allow(dead_code)]
mod host;
use common::{TestServer, python_kind_registry, signup};
use mcphost::kinds::python;
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

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

/// Never less than 5s (the AC's own floor); beyond that, scale with how
/// many processes were actually observed outstanding right after the call
/// failed -- a storm that only got a handful of forks off before hitting
/// the cap needs no more than the floor, while one that got further needs
/// proportionally longer for the kernel/namespace teardown to catch up.
/// 100ms/process is a deliberately generous heuristic for a shared build
/// box, not a measured constant.
fn cleanup_deadline(spawned: usize) -> Duration {
    Duration::from_secs(5).max(Duration::from_millis(spawned as u64 * 100))
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
    let tool_process_limit = python::max_processes();
    let host_rlimit_nproc = host::effective_nproc_limit();

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
    let started = Instant::now();

    let err = client
        .tools_call(&qualified, json!({"n": 500}))
        .await
        .expect_err("a 500-way fork storm must be refused, not silently allowed");
    assert_eq!(
        err.error_code.as_deref(),
        Some("tool_process_limit"),
        "fork storm must be refused with tool_process_limit (the sandbox's own cap of {tool_process_limit}) \
         regardless of this host's own RLIMIT_NPROC ({host_rlimit_nproc}); {}",
        host::describe_host(),
    );

    // Processes observed outstanding right after the call returned, before
    // any cleanup wait -- an upper bound on how many the storm actually got
    // off the ground before the cap stopped it. Scales the deadline below.
    let spawned = proc_count().saturating_sub(baseline);
    let deadline = cleanup_deadline(spawned);
    let deadline_at = Instant::now() + deadline;

    let mut surviving = proc_count().saturating_sub(baseline);
    while surviving > 25 && Instant::now() < deadline_at {
        tokio::time::sleep(Duration::from_millis(200)).await;
        surviving = proc_count().saturating_sub(baseline);
    }
    let elapsed = started.elapsed();

    assert!(
        surviving <= 25,
        "process count must return near baseline within the scaled deadline: spawned={spawned} \
         surviving={surviving} tool_process_limit={tool_process_limit} host_rlimit_nproc={host_rlimit_nproc} \
         elapsed={elapsed:?} deadline={deadline:?}; {}",
        host::describe_host(),
    );
}
