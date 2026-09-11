//! Shared host-fact helpers for host-sensitive assertions
//! (PRD-mcphost-tests-host-independence). A test whose verdict must hold on
//! any host (a wall-clock budget, a process cap, a unit tree) reads its
//! facts through here and prints [`describe_host`] on failure, so a red run
//! always shows the host it ran on, not just the number it saw.
//!
//! Not a crate module: each `tests/*.rs` file is its own crate root, so
//! this is pulled in per-file with `#[path = "support/host.rs"] mod host;`
//! rather than a shared `tests/support/mod.rs` — see the doc comment on
//! any consuming test file for why.

use std::fs;

/// Host facts captured once per assertion: hostname, `nproc`, the 1-minute
/// load average, this process's soft `RLIMIT_NPROC`, and the real uid.
/// Requirement 4.
#[derive(Debug, Clone)]
pub struct HostFacts {
    pub hostname: String,
    pub nproc: usize,
    pub load1: f64,
    pub rlimit_nproc: u64,
    pub uid: u32,
}

/// `load1 / nproc`, floored at `1.0` -- an idle host (load at or below its
/// own core count) never widens the budget past the base one; only genuine
/// contention does. Requirement 1's `infer_ac14` multiplies its 100ms
/// budget by this.
pub fn load_multiplier() -> f64 {
    let load1 = read_load1();
    let n = nproc() as f64;
    if n <= 0.0 {
        return 1.0;
    }
    (load1 / n).max(1.0)
}

/// The process cap actually in force for this uid: the soft `RLIMIT_NPROC`
/// read via `getrlimit`. Deliberately separate from mcphost's own
/// tool-level process cap (`kinds::python::max_processes`) -- requirement 2
/// is explicit that a test must never conflate "how many processes this
/// user may run" with "how many processes mcphost lets one sandboxed call
/// spawn"; the two numbers differ by three orders of magnitude on a real
/// build box (232241 vs 64) and only the latter is what the fork-storm test
/// asserts against.
pub fn effective_nproc_limit() -> u64 {
    // SAFETY: `rlim` is a plain, zero-initialized `libc::rlimit` (two
    // `u64`/`rlim_t` fields, no invariants beyond being initialized data);
    // `getrlimit` only ever writes into it and returns a status code.
    unsafe {
        let mut rlim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NPROC, &mut rlim) == 0 {
            rlim.rlim_cur as u64
        } else {
            0
        }
    }
}

fn nproc() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn read_load1() -> f64 {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn hostname_string() -> String {
    // No extra dependency for this: `hostname(1)` is present on every host
    // this suite runs on (RedBaron and the burst box alike); `$HOSTNAME` is
    // the fallback for a shell that doesn't export the binary on `PATH`.
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown-host".to_string())
}

/// Snapshot every fact in [`HostFacts`] at once.
pub fn collect() -> HostFacts {
    HostFacts {
        hostname: hostname_string(),
        nproc: nproc(),
        load1: read_load1(),
        rlimit_nproc: effective_nproc_limit(),
        // SAFETY: getuid() takes no arguments and cannot fail.
        uid: unsafe { libc::getuid() },
    }
}

/// One line of host facts, meant to be embedded directly in a failure
/// message so a red run on an unfamiliar host explains itself without a
/// follow-up SSH session. Requirement 4.
pub fn describe_host() -> String {
    let f = collect();
    format!(
        "host={} nproc={} load1={:.2} rlimit_nproc={} uid={}",
        f.hostname, f.nproc, f.load1, f.rlimit_nproc, f.uid
    )
}
