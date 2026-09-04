//! The sandboxed subprocess runner protocol (PRD-mcphost-code-tools
//! requirement 4/5, technical considerations): spawn one child process per
//! call, isolated from the host and from every other tenant, with CPU/
//! memory/file/timeout limits enforced by the kernel rather than trusted
//! code inside the child. Deliberately independent of any one `Kind` (the
//! PRD's own words: "so the future `wasm` kind ... can reuse metering and
//! error mapping") -- this module knows how to run *a* command under
//! resource limits and isolation and report back what happened; it has no
//! notion of Python, JSON-RPC tool envelopes, or `main(args)`. `src/kinds/
//! python.rs` is this module's first, and so far only, caller.
//!
//! ## Isolation mechanism
//!
//! Two mechanisms are supported, in the PRD's stated preference order:
//! `bwrap` (bubblewrap) if the binary is on `$PATH`, else `unshare -Urn`
//! plus `setpriv`. Both put the child in a fresh user namespace mapped to an
//! unprivileged uid/gid distinct from the service user (requirement 5), a
//! fresh mount namespace where only the scratch directory and the venv are
//! visible, and (for `network: none`) a fresh, unconfigured network
//! namespace. [`detect_mechanism`] probes once at process start; the result
//! is what `/healthz` and the `python` kind's own `mechanism()` report.
//!
//! ## Resource accounting
//!
//! `bwrap`/`unshare` fork an inner supervisor process that itself execs (or
//! forks again for) the real interpreter -- the pid `tokio::process::Command`
//! hands back is that outer supervisor's, not the workload's. Rather than
//! fight `tokio`'s own child-reaping (calling `wait4` on the same pid `tokio`
//! is separately waiting on is a documented source of races), this module
//! periodically walks `/proc/<pid>/task/*/children` to find every live
//! descendant and sums their CPU ticks and RSS -- a few-millisecond-grained
//! approximation, not exact kernel-accounted `rusage`, but sufficient for
//! the PRD's metering requirement (a `calls` row with plausible cpu/memory)
//! and immune to any reaper race, since it never calls `wait()`/`wait4()`
//! itself.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};

/// How this box isolates a sandboxed child. Recorded in `/healthz`
/// (requirement 5: "the chosen mechanism is recorded in `/healthz` and the
/// README").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationMechanism {
    Bwrap,
    UnshareSetpriv,
    /// Neither `bwrap` nor `unshare`+`setpriv` is on `$PATH`. Calls still
    /// run (rlimits and a scratch cwd still apply), but network and
    /// filesystem isolation are not enforced -- a degraded mode that should
    /// never occur on a box `mcphost-deploy` provisioned, and is exercised
    /// in this crate's own tests only via an explicit opt-in constructor.
    None,
}

impl IsolationMechanism {
    pub fn as_str(self) -> &'static str {
        match self {
            IsolationMechanism::Bwrap => "bwrap",
            IsolationMechanism::UnshareSetpriv => "unshare+setpriv",
            IsolationMechanism::None => "none",
        }
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

/// Probe once for the best available mechanism, in the PRD's stated order.
pub fn detect_mechanism() -> IsolationMechanism {
    if on_path("bwrap") {
        IsolationMechanism::Bwrap
    } else if on_path("unshare") && on_path("setpriv") {
        IsolationMechanism::UnshareSetpriv
    } else {
        IsolationMechanism::None
    }
}

/// `true` if this process can create an unprivileged user namespace.
///
/// GitHub Actions runners (and some other CI/container environments) deny
/// unprivileged `CLONE_NEWUSER`, which both isolation mechanisms depend on
/// (see the module doc's "Isolation mechanism" section) -- `bwrap` and
/// `unshare -Urn` both fail there even when the binaries are on `$PATH`.
/// The handful of tests that actually spawn a sandboxed child use this
/// probe to skip cleanly in that environment rather than fail; on any box
/// that does support user namespaces (every `mcphost-deploy`-provisioned
/// host, and this developer's machine) they keep running for real.
pub fn supports_user_namespaces() -> bool {
    std::process::Command::new("unshare")
        .args(["--user", "--map-root-user", "--", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// `true` if a python-kind sandbox test should skip on this box, `false` if
/// it should run for real.
///
/// [`supports_user_namespaces`] is a live capability probe, not a CI check
/// -- calling it directly as the skip condition (the v0.5.2/v0.5.3 pattern)
/// means ANY box lacking unprivileged user namespaces silently no-ops the
/// whole python-kind sandbox suite, including this repository's own
/// designated build machine when it happens to lack them, not just
/// GitHub's hosted runners. That is indistinguishable from the suite
/// passing for real, so a real regression (dropped network isolation, a
/// broken filesystem allowlist) can ship with `cargo test --workspace`
/// reporting 0 failed.
///
/// This gates the skip on both signals: only inside CI (`$CI` is set --
/// GitHub Actions exports `CI=true` on every hosted runner) does a missing
/// probe mean "skip cleanly". Anywhere else, a missing probe is a broken
/// or misconfigured environment, not something to skip past quietly, so
/// this panics with a message naming the fix instead of returning `true`.
pub fn require_user_namespaces_or_ci_skip() -> bool {
    if supports_user_namespaces() {
        return false;
    }
    if std::env::var_os("CI").is_some() {
        return true;
    }
    panic!(
        "python-kind sandbox tests require unprivileged user namespaces; \
         refusing to silently skip outside CI. `unshare --user --map-root-user -- true` \
         failed on this box and $CI is not set. If this really is a CI runner, set CI=true; \
         otherwise fix unprivileged user namespaces (e.g. `sysctl kernel.unprivileged_userns_clone=1`) \
         or run on a box that has them."
    );
}

/// The unprivileged uid/gid a sandboxed child runs as inside its fresh user
/// namespace -- `nobody`/`nogroup` on every Linux distribution `mcphost`
/// targets, and (requirement 5) distinct from whatever uid runs `mcphost`
/// itself.
const SANDBOX_UID: u32 = 65534;
const SANDBOX_GID: u32 = 65534;

/// Kernel resource limits applied via `setrlimit` in `pre_exec`, before the
/// isolation wrapper (`bwrap`/`unshare`) is even exec'd -- rlimits survive
/// every `execve` in the chain, so setting them once here bounds the whole
/// tree down to the interpreter.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub cpu_seconds: u64,
    pub memory_mb: u64,
    pub max_open_files: u64,
    pub max_file_size_mb: u64,
}

/// `network: none` (default) vs `network: public` (requirement 5: the same
/// private-address rules as the `http` kind, through a host-provided proxy).
/// This crate does not itself ship that proxy -- `Public`'s `http_proxy` /
/// `https_proxy` are only set as `HTTP_PROXY`/`HTTPS_PROXY` in the child's
/// environment when the operator has configured one
/// (`$MCPHOST_EGRESS_PROXY`); with no proxy configured, `Public` grants full
/// outbound network access with no SSRF filtering. Documented, scoped gap:
/// no acceptance criterion in this PRD exercises `network: public`'s
/// filtering, only `network: none`'s block (AC8).
#[derive(Debug, Clone)]
pub enum NetworkMode {
    None,
    Public { http_proxy: Option<String> },
}

/// One sandboxed run: an interpreter, a script to run it against, a
/// scratch (writable) directory, zero or more read-only directories the
/// script needs visible (a venv, in `python`'s case), a JSON/bytes payload
/// on stdin, and the limits/isolation to run it under.
pub struct RunSpec {
    pub interpreter: PathBuf,
    pub interpreter_args: Vec<String>,
    pub script_path: PathBuf,
    pub scratch_dir: PathBuf,
    pub read_only_dirs: Vec<PathBuf>,
    pub stdin_payload: Vec<u8>,
    pub limits: ResourceLimits,
    pub wall_clock_timeout: Duration,
    pub network: NetworkMode,
    /// `(name, value)` pairs set in the child's environment (e.g.
    /// `SECRET_TOKEN`) -- everything else is cleared (requirement 5: "no
    /// environment variables inherited").
    pub extra_env: Vec<(String, String)>,
    pub isolation: IsolationMechanism,
}

/// Tails kept for error reporting (requirement 6: "the last 2 KiB of stdout
/// and stderr").
const TAIL_BYTES: usize = 2048;
/// Hard cap on how much of a child's stdout/stderr this module buffers at
/// all, regardless of the reported tail size -- bounds memory for a chatty
/// or runaway child without needing the isolation layer to enforce it.
const READ_CAP_BYTES: usize = 256 * 1024;

fn tail_str(buf: &[u8]) -> String {
    let start = buf.len().saturating_sub(TAIL_BYTES);
    String::from_utf8_lossy(&buf[start..]).into_owned()
}

/// What happened, independent of any envelope convention a particular
/// language runner writes into stdout -- `python.rs` (or a future `wasm.rs`)
/// interprets `stdout_tail` further to distinguish e.g. an ordinary
/// exception from an OOM.
#[derive(Debug, Clone)]
pub enum SandboxOutcome {
    /// Exited 0.
    Exited {
        stdout: Vec<u8>,
        stderr_tail: String,
        cpu_ms: i64,
        peak_rss_kb: i64,
    },
    /// Exited non-zero.
    NonZeroExit {
        code: i32,
        stdout_tail: String,
        stderr_tail: String,
        cpu_ms: i64,
        peak_rss_kb: i64,
    },
    /// Killed by a signal other than this module's own timeout kill.
    Signaled {
        signal: i32,
        stdout_tail: String,
        stderr_tail: String,
        cpu_ms: i64,
        peak_rss_kb: i64,
    },
    /// The wall-clock cap fired; the process group was killed
    /// (requirement 4: "the group is killed on timeout").
    TimedOut { cpu_ms: i64, peak_rss_kb: i64 },
}

#[derive(Debug, Clone, Copy, Default)]
struct UsageSample {
    cpu_ms: i64,
    rss_kb: i64,
}

/// Every clock-tick field's index in `/proc/<pid>/stat` (1-based per
/// `proc(5)`; `utime`/`stime` are fields 14/15). Parsed by splitting on the
/// closing paren of the (possibly space-containing) comm field first, since
/// that's the one field that can itself contain whitespace.
fn read_proc_stat_cpu_ticks(pid: i32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = text.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_comm.split(' ').collect();
    // fields[0] is field 3 (state); utime is field 14 -> index 11 here,
    // stime is field 15 -> index 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

fn read_proc_status_rss_kb(pid: i32) -> Option<i64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.trim().split(' ').next()?.parse().ok();
        }
    }
    None
}

fn read_proc_children(pid: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path().join("children")) else {
            continue;
        };
        out.extend(
            text.split_whitespace()
                .filter_map(|s| s.parse::<i32>().ok()),
        );
    }
    out
}

fn clk_tck() -> i64 {
    // SAFETY: sysconf with a valid, well-known name; no pointers involved.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks } else { 100 }
}

/// Sums CPU time and RSS across `root_pid` and every live descendant
/// (see module docs: the outer pid `tokio` gives us is a supervisor, not
/// the workload). Best-effort: a pid that's already exited between the
/// children-listing read and the stat/status read is simply skipped.
fn sample_usage(root_pid: i32) -> UsageSample {
    let mut stack = vec![root_pid];
    let mut seen = HashSet::new();
    let mut total_ticks: u64 = 0;
    let mut total_rss_kb: i64 = 0;
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(ticks) = read_proc_stat_cpu_ticks(pid) {
            total_ticks += ticks;
        }
        if let Some(rss) = read_proc_status_rss_kb(pid) {
            total_rss_kb += rss;
        }
        stack.extend(read_proc_children(pid));
    }
    let tck = clk_tck();
    UsageSample {
        cpu_ms: (total_ticks as i64) * 1000 / tck,
        rss_kb: total_rss_kb,
    }
}

/// `setsid()` + the rlimits, run in the forked child before `execve` of the
/// isolation wrapper (or, with [`IsolationMechanism::None`], the interpreter
/// itself). `setsid` makes this process its own session and process-group
/// leader, so a timeout can `killpg` the whole tree in one call.
///
/// # Safety
/// Called by the `tokio`/`std` process machinery strictly between `fork`
/// and `exec` (that's what `pre_exec` guarantees): only async-signal-safe
/// libc calls are made here, no allocation, no locks.
unsafe fn pre_exec_setup(limits: ResourceLimits) -> std::io::Result<()> {
    // SAFETY: called strictly between fork and exec by the pre_exec hook; only async-signal-safe libc calls follow, no allocation, no locks.
    unsafe {
        if libc::setsid() == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let set = |resource: u32, limit: u64| -> std::io::Result<()> {
            let rl = libc::rlimit {
                rlim_cur: limit,
                rlim_max: limit,
            };
            if libc::setrlimit(resource, &rl) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        };
        set(libc::RLIMIT_CPU, limits.cpu_seconds)?;
        set(libc::RLIMIT_AS, limits.memory_mb * 1024 * 1024)?;
        set(libc::RLIMIT_NOFILE, limits.max_open_files)?;
        set(libc::RLIMIT_FSIZE, limits.max_file_size_mb * 1024 * 1024)?;
        set(libc::RLIMIT_CORE, 0)?;
    }
    Ok(())
}

/// Builds the `bwrap` argv: only `read_only_dirs` (the interpreter's own
/// tree plus the venv) and `scratch_dir` (writable) are visible; every
/// other host path, including the host's own database and every other
/// tenant's env directory, simply does not exist inside the sandbox
/// (requirement 9).
fn bwrap_command(spec: &RunSpec) -> Command {
    let mut cmd = Command::new("bwrap");
    cmd.arg("--unshare-all");
    if let NetworkMode::Public { .. } = spec.network {
        cmd.arg("--share-net");
    }
    cmd.args(["--uid", &SANDBOX_UID.to_string()]);
    cmd.args(["--gid", &SANDBOX_GID.to_string()]);
    cmd.arg("--clearenv");
    for (key, value) in &spec.extra_env {
        cmd.args(["--setenv", key, value]);
    }
    cmd.args(["--proc", "/proc"]);
    cmd.args(["--dev", "/dev"]);
    // `--tmpfs /tmp` must come *before* every other bind below: bwrap
    // applies mount operations in argv order, and both `scratch_dir` and
    // (in every test, and any production box whose `$MCPHOST_DATA_DIR` is
    // under `/tmp`) `read_only_dirs`' env directory live under `/tmp` --
    // binding either first and mounting a fresh tmpfs over `/tmp` second
    // would shadow (hide) the bind, the exact "Can't chdir to <scratch>: No
    // such file or directory" / "execvp <env>/bin/python: No such file or
    // directory" bugs this ordering avoids.
    cmd.args(["--tmpfs", "/tmp"]);
    for dir in &spec.read_only_dirs {
        cmd.arg("--ro-bind");
        cmd.arg(dir);
        cmd.arg(dir);
    }
    cmd.arg("--bind");
    cmd.arg(&spec.scratch_dir);
    cmd.arg(&spec.scratch_dir);
    cmd.args(["--chdir", &spec.scratch_dir.to_string_lossy()]);
    cmd.arg("--die-with-parent");
    cmd.arg("--new-session");
    cmd.arg("--");
    cmd.arg(&spec.interpreter);
    cmd.args(&spec.interpreter_args);
    cmd.arg(&spec.script_path);
    cmd
}

/// Fallback mechanism (technical considerations: "`unshare -Urn` plus
/// `setpriv`"). Weaker than `bwrap`: no per-path mount allowlist, so
/// filesystem isolation relies on the sandboxed uid's own file permissions
/// rather than the path simply not existing. Not exercised by this box's
/// own tests (`bwrap` is present here and wins per [`detect_mechanism`]);
/// kept as the PRD-specified degrade path for a box without `bwrap`.
fn unshare_setpriv_command(spec: &RunSpec) -> Command {
    let mut cmd = Command::new("unshare");
    cmd.arg("--user");
    cmd.arg("--map-root-user");
    cmd.arg("--mount");
    if matches!(spec.network, NetworkMode::None) {
        cmd.arg("--net");
    }
    cmd.arg("--");
    cmd.arg("setpriv");
    cmd.args(["--reuid", &SANDBOX_UID.to_string()]);
    cmd.args(["--regid", &SANDBOX_GID.to_string()]);
    cmd.arg("--clear-groups");
    cmd.arg("--");
    cmd.arg(&spec.interpreter);
    cmd.args(&spec.interpreter_args);
    cmd.arg(&spec.script_path);
    cmd.current_dir(&spec.scratch_dir);
    cmd.env_clear();
    for (key, value) in &spec.extra_env {
        cmd.env(key, value);
    }
    cmd
}

fn no_isolation_command(spec: &RunSpec) -> Command {
    let mut cmd = Command::new(&spec.interpreter);
    cmd.args(&spec.interpreter_args);
    cmd.arg(&spec.script_path);
    cmd.current_dir(&spec.scratch_dir);
    cmd.env_clear();
    for (key, value) in &spec.extra_env {
        cmd.env(key, value);
    }
    cmd
}

/// Picks the argv/env for `spec.isolation`, shared by [`run`] (one-shot) and
/// [`spawn_persistent`] (PRD-mcphost-code-tools-warm-pool: a long-lived
/// sandbox serving more than one call) -- the isolation wrapper's argv
/// doesn't know or care whether its stdin will be closed after one line or
/// kept open for many.
fn build_isolated_command(spec: &RunSpec) -> Command {
    match spec.isolation {
        IsolationMechanism::Bwrap => bwrap_command(spec),
        IsolationMechanism::UnshareSetpriv => unshare_setpriv_command(spec),
        IsolationMechanism::None => no_isolation_command(spec),
    }
}

/// Reads `reader` to EOF (or [`READ_CAP_BYTES`], whichever comes first) into
/// a growable buffer. Draining past the cap keeps consuming the pipe (so a
/// chatty child doesn't block on a full pipe buffer) without retaining the
/// extra bytes.
async fn drain_capped(mut reader: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < READ_CAP_BYTES {
                    let take = n.min(READ_CAP_BYTES - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                }
            }
            Err(_) => break,
        }
    }
    buf
}

/// Polls [`sample_usage`] on a timer until told to stop, keeping the peak
/// RSS seen and the latest (monotonically non-decreasing) CPU time.
async fn poll_usage(
    root_pid: i32,
    usage: Arc<Mutex<UsageSample>>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    loop {
        let sample = sample_usage(root_pid);
        if let Ok(mut guard) = usage.lock() {
            guard.rss_kb = guard.rss_kb.max(sample.rss_kb);
            guard.cpu_ms = guard.cpu_ms.max(sample.cpu_ms);
        }
        tokio::select! {
            _ = &mut stop => break,
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

/// Runs `spec` to completion (or until its wall-clock cap fires) and
/// classifies the result. Never panics on a misbehaving child; a spawn
/// failure (missing interpreter, `bwrap` not installed despite
/// `IsolationMechanism::Bwrap` being reported, ...) is the only `Err` path,
/// reported as [`std::io::Error`] for the caller to wrap in its own error
/// taxonomy.
pub async fn run(spec: RunSpec) -> std::io::Result<SandboxOutcome> {
    let mut cmd = build_isolated_command(&spec);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    let limits = spec.limits;
    // `pre_exec`'s own contract (see `pre_exec_setup`'s doc comment) guarantees
    // this closure runs strictly between fork and exec.
    // SAFETY: pre_exec_setup upholds the fork/exec-window contract documented on it.
    unsafe {
        cmd.pre_exec(move || pre_exec_setup(limits));
    }

    let mut child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("spawned child reported no pid"))?
        as i32;

    let mut stdin = child.stdin.take();
    let stdin_payload = spec.stdin_payload.clone();
    let stdin_write = tokio::spawn(async move {
        if let Some(mut s) = stdin.take() {
            let _ = s.write_all(&stdin_payload).await;
            // Dropping `s` here closes the pipe (EOF for the child).
        }
    });

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("stdout not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("stderr not piped"))?;
    let stdout_task = tokio::spawn(drain_capped(stdout));
    let stderr_task = tokio::spawn(drain_capped(stderr));

    let usage = Arc::new(Mutex::new(UsageSample::default()));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let poll_task = tokio::spawn(poll_usage(pid, usage.clone(), stop_rx));

    let _ = stdin_write.await;
    let wait_result = tokio::time::timeout(spec.wall_clock_timeout, child.wait()).await;
    let timed_out = wait_result.is_err();
    let status = if timed_out {
        // `pid` is this call's own sandboxed process-group leader (set via
        // `pre_exec_setup`'s `setsid`); killing its group cannot affect any
        // other process on the box.
        // SAFETY: pid is this sandbox run's own process-group leader; see above.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
        child.wait().await.ok()
    } else {
        wait_result.ok().and_then(Result::ok)
    };

    let _ = stop_tx.send(());
    let _ = poll_task.await;
    let final_sample = usage.lock().map(|g| *g).unwrap_or_default();
    let stdout_buf = stdout_task.await.unwrap_or_default();
    let stderr_buf = stderr_task.await.unwrap_or_default();
    let stderr_tail = tail_str(&stderr_buf);

    if timed_out {
        return Ok(SandboxOutcome::TimedOut {
            cpu_ms: final_sample.cpu_ms,
            peak_rss_kb: final_sample.rss_kb,
        });
    }

    use std::os::unix::process::ExitStatusExt;
    let status = status.ok_or_else(|| std::io::Error::other("child exited with no status"))?;
    if let Some(signal) = status.signal() {
        return Ok(SandboxOutcome::Signaled {
            signal,
            stdout_tail: tail_str(&stdout_buf),
            stderr_tail,
            cpu_ms: final_sample.cpu_ms,
            peak_rss_kb: final_sample.rss_kb,
        });
    }
    match status.code() {
        Some(0) => Ok(SandboxOutcome::Exited {
            stdout: stdout_buf,
            stderr_tail,
            cpu_ms: final_sample.cpu_ms,
            peak_rss_kb: final_sample.rss_kb,
        }),
        Some(code) => Ok(SandboxOutcome::NonZeroExit {
            code,
            stdout_tail: tail_str(&stdout_buf),
            stderr_tail,
            cpu_ms: final_sample.cpu_ms,
            peak_rss_kb: final_sample.rss_kb,
        }),
        None => Ok(SandboxOutcome::Signaled {
            signal: 0,
            stdout_tail: tail_str(&stdout_buf),
            stderr_tail,
            cpu_ms: final_sample.cpu_ms,
            peak_rss_kb: final_sample.rss_kb,
        }),
    }
}

// ---- persistent sandbox (PRD-mcphost-code-tools-warm-pool) -----------------
//
// A long-lived counterpart to `run`: the same isolation wrapper and rlimits,
// spawned once and kept idle between calls rather than exiting after one.
// The runner script on the other end (`kinds::python`'s `PY_RUNNER_SCRIPT`)
// reads one JSON request per line from stdin and writes one JSON response
// per line to stdout in a loop, so this module's job shrinks to "write a
// line, read a line, with a timeout" -- everything else (rlimits, the
// process-group kill on timeout, the `/proc` usage sampling) is identical to
// `run`'s one-shot path, just amortized across many calls instead of one.

/// What one [`PersistentSandbox::call`] returned.
#[derive(Debug)]
pub enum PersistentCallOutcome {
    /// One full response line (without its trailing newline).
    Responded {
        line: Vec<u8>,
        cpu_ms: i64,
        peak_rss_kb: i64,
    },
    /// No response within the caller's deadline. The sandbox is still
    /// running (mid-call) and must be killed -- there is no way to know
    /// which future line, if any, would have answered this call, so it can
    /// never be handed to a later caller (PRD requirement 2 / AC5: "the
    /// pool no longer holds it").
    TimedOut,
    /// The child exited (or its stdout pipe closed) before answering --
    /// crashed, hit its own rlimit, or was killed out of band. Also fatal to
    /// this sandbox instance.
    Closed,
}

/// A spawned, still-running sandboxed child, communicating one JSON request
/// per line in on stdin and one JSON response per line out on stdout. Owns
/// its own stderr-draining and usage-sampling background tasks so a caller
/// juggling many of these (the warm pool) doesn't have to.
pub struct PersistentSandbox {
    child: tokio::process::Child,
    pid: i32,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    _stderr_task: tokio::task::JoinHandle<()>,
    /// Cumulative CPU ms as of the last call (or spawn, for the first one) --
    /// each [`PersistentSandbox::call`] reports only the delta since this,
    /// since `RLIMIT_CPU`-style accounting is itself cumulative for the
    /// process's whole life, not per call.
    baseline_cpu_ms: i64,
}

/// Drains `reader` into `buf` continuously (not just once): a persistent
/// child's stderr can be written to across many calls, and this task must
/// keep the pipe from filling for the sandbox's entire lifetime, not just
/// one call's.
async fn drain_capped_into(mut reader: impl tokio::io::AsyncRead + Unpin, buf: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Ok(mut guard) = buf.lock() {
                    guard.extend_from_slice(&chunk[..n]);
                    if guard.len() > READ_CAP_BYTES {
                        let start = guard.len() - READ_CAP_BYTES;
                        guard.drain(0..start);
                    }
                }
            }
        }
    }
}

/// Spawns `spec.interpreter`/`spec.script_path` under `spec.isolation` and
/// leaves it running, stdin/stdout/stderr all piped, ready for
/// [`PersistentSandbox::call`]. `spec.stdin_payload` is ignored here (a
/// persistent sandbox's first request comes from the first `call`, not from
/// spawn time); every other `RunSpec` field is honored exactly as `run`
/// honors it.
pub async fn spawn_persistent(spec: &RunSpec) -> std::io::Result<PersistentSandbox> {
    let mut cmd = build_isolated_command(spec);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    let limits = spec.limits;
    // SAFETY: pre_exec_setup upholds the fork/exec-window contract documented on it.
    unsafe {
        cmd.pre_exec(move || pre_exec_setup(limits));
    }

    let mut child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("spawned child reported no pid"))?
        as i32;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("stdin not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("stdout not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("stderr not piped"))?;
    let stderr_tail = Arc::new(Mutex::new(Vec::new()));
    let stderr_task = tokio::spawn(drain_capped_into(stderr, stderr_tail.clone()));

    Ok(PersistentSandbox {
        child,
        pid,
        stdin,
        stdout: BufReader::new(stdout).lines(),
        stderr_tail,
        _stderr_task: stderr_task,
        baseline_cpu_ms: 0,
    })
}

impl PersistentSandbox {
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// The last known stderr tail (up to [`READ_CAP_BYTES`]), as of whenever
    /// this is called -- a snapshot, not consumed.
    pub fn stderr_tail(&self) -> String {
        let buf = self.stderr_tail.lock().map(|g| g.clone()).unwrap_or_default();
        tail_str(&buf)
    }

    /// Writes one line (`payload` plus a trailing `\n`) to the sandbox's
    /// stdin and waits up to `timeout` for one line back. `Ok` always --
    /// an `Err` here is a real spawn/IO-layer failure distinct from a
    /// timeout or a closed pipe, both of which are ordinary
    /// [`PersistentCallOutcome`] variants the caller (the warm pool) is
    /// expected to handle by evicting this sandbox.
    pub async fn call(
        &mut self,
        payload: &[u8],
        timeout: Duration,
    ) -> std::io::Result<PersistentCallOutcome> {
        if self.stdin.write_all(payload).await.is_err() || self.stdin.write_all(b"\n").await.is_err()
        {
            return Ok(PersistentCallOutcome::Closed);
        }
        if self.stdin.flush().await.is_err() {
            return Ok(PersistentCallOutcome::Closed);
        }

        match tokio::time::timeout(timeout, self.stdout.next_line()).await {
            Ok(Ok(Some(line))) => {
                let sample = sample_usage(self.pid);
                let cpu_ms = (sample.cpu_ms - self.baseline_cpu_ms).max(0);
                self.baseline_cpu_ms = sample.cpu_ms;
                Ok(PersistentCallOutcome::Responded {
                    line: line.into_bytes(),
                    cpu_ms,
                    peak_rss_kb: sample.rss_kb,
                })
            }
            Ok(Ok(None)) => Ok(PersistentCallOutcome::Closed),
            Ok(Err(_)) => Ok(PersistentCallOutcome::Closed),
            Err(_elapsed) => Ok(PersistentCallOutcome::TimedOut),
        }
    }

    /// Kills this sandbox's whole process group (requirement 2: "killed on
    /// the 30s deadline like a cold one") and reaps it. Consumes `self` --
    /// once killed, a sandbox is never reused (the pool must not hand out a
    /// dead or dying entry).
    pub async fn kill(mut self) {
        // SAFETY: pid is this sandbox's own process-group leader (set via
        // pre_exec_setup's setsid), same as run()'s timeout-kill path.
        unsafe {
            libc::killpg(self.pid, libc::SIGKILL);
        }
        let _ = self.child.wait().await;
    }
}

/// `true` if `path`'s parent chain, up to and including `root`, is entirely
/// inside `root` -- a small guard `python.rs` uses before treating a
/// caller-influenced path (a scratch/env directory name) as safe to create.
pub fn is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_real_mechanism_on_this_box() {
        // This crate's CI/dev boxes ship `bwrap` (mcphost-deploy installs
        // it); asserting the probe finds *something* keeps this test
        // meaningful without hard-coding which mechanism (a box without
        // `bwrap` but with `unshare`+`setpriv` is also a valid target).
        assert_ne!(detect_mechanism(), IsolationMechanism::None);
    }

    #[tokio::test]
    async fn runs_a_trivial_script_and_reports_exit_0() {
        if !supports_user_namespaces() {
            println!("skipped: no user namespaces");
            return;
        }
        let scratch =
            std::env::temp_dir().join(format!("mcphost-sandbox-test-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let script = scratch.join("script.py");
        std::fs::write(&script, "import sys; sys.stdout.write(sys.stdin.read())")
            .expect("write script");

        let spec = RunSpec {
            interpreter: PathBuf::from("/usr/bin/python3"),
            interpreter_args: vec!["-I".to_string(), "-S".to_string()],
            script_path: script,
            scratch_dir: scratch.clone(),
            read_only_dirs: ["/usr", "/lib", "/lib64", "/bin"]
                .into_iter()
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .collect(),
            stdin_payload: b"hello".to_vec(),
            limits: ResourceLimits {
                cpu_seconds: 5,
                memory_mb: 256,
                max_open_files: 64,
                max_file_size_mb: 16,
            },
            wall_clock_timeout: Duration::from_secs(7),
            network: NetworkMode::None,
            extra_env: vec![],
            isolation: detect_mechanism(),
        };

        let outcome = run(spec).await.expect("sandbox run must not error");
        let _ = std::fs::remove_dir_all(&scratch);
        match outcome {
            SandboxOutcome::Exited { stdout, .. } => assert_eq!(stdout, b"hello"),
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn kills_the_group_on_timeout() {
        if !supports_user_namespaces() {
            println!("skipped: no user namespaces");
            return;
        }
        let scratch =
            std::env::temp_dir().join(format!("mcphost-sandbox-timeout-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let script = scratch.join("script.py");
        std::fs::write(&script, "import time; time.sleep(30)").expect("write script");

        let spec = RunSpec {
            interpreter: PathBuf::from("/usr/bin/python3"),
            interpreter_args: vec!["-I".to_string(), "-S".to_string()],
            script_path: script,
            scratch_dir: scratch.clone(),
            read_only_dirs: ["/usr", "/lib", "/lib64", "/bin"]
                .into_iter()
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .collect(),
            stdin_payload: vec![],
            limits: ResourceLimits {
                cpu_seconds: 30,
                memory_mb: 256,
                max_open_files: 64,
                max_file_size_mb: 16,
            },
            wall_clock_timeout: Duration::from_millis(500),
            network: NetworkMode::None,
            extra_env: vec![],
            isolation: detect_mechanism(),
        };

        let started = std::time::Instant::now();
        let outcome = run(spec).await.expect("sandbox run must not error");
        let _ = std::fs::remove_dir_all(&scratch);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must return promptly after killing the group"
        );
        assert!(
            matches!(outcome, SandboxOutcome::TimedOut { .. }),
            "expected TimedOut, got {outcome:?}"
        );
    }
}
