//! `mcphost migrate --check-compat` (PRD-mcphost-migration-safety
//! requirement 2): proves the *previous* release's binary can still run
//! against *this* release's migrated schema before a redeploy switches to
//! it, without ever mutating the live database.
//!
//! The flow: copy the live database file to a private scratch directory
//! (`VACUUM INTO`, a hot copy off a read-only connection that works
//! correctly under WAL), apply this binary's own pending migrations to the
//! copy (the same idempotent path `serve`/`migrate` already run, just
//! pointed at the scratch copy), spawn the *previous* release's binary
//! against that copy on a loopback port, and run the same four checks a
//! human would: `initialize`, `tools/list`, one `tools/call` of a
//! control-plane tool (`host.whoami`, reached via a throwaway signup on the
//! copy), and `GET /healthz`. `Ok(())` on success; a named failing step
//! ([`CompatCheckFailure::step`]) otherwise. The scratch directory --
//! including the copied database -- is removed either way; the live
//! database is opened read-only and never written to.
//!
//! PRD-mcphost-checkcompat-port-race: the port the previous release binds
//! is never merely "chosen" and hoped for -- [`bind_loopback_listener`]
//! binds `127.0.0.1:0` and keeps the listener open until it hands the very
//! same socket to the child as an inherited fd (`LISTEN_FDS=1`/
//! `LISTEN_PID=<child>`, the systemd socket-activation convention; see
//! `src/http.rs`'s `inherited_listener`), so no other process can ever
//! bind that exact port in between (requirement 1). A random
//! `MCPHOST_COMPAT_TOKEN` travels with the child in its env and
//! [`wait_ready`] only accepts a `/healthz` response carrying that token
//! back in `X-Mcphost-Compat-Token` (requirement 2) -- a foreign process
//! answering on the same address can no longer be mistaken for the child
//! this check actually spawned. `wait_ready` also polls the child's own
//! liveness (`Child::try_wait`) every iteration, so a previous release that
//! never comes up (like `/bin/false`) fails in one poll interval instead of
//! after a 10s timeout (requirement 3).
//!
//! PRD-mcphost-compat-check-unprivileged (2026-10-03): `mcphost-deploy`
//! runs this whole subcommand over ssh as root, so `spawn_previous`'s
//! child used to inherit root's own real uid -- which trips
//! `sandbox::refuse_to_serve_as_root` (`src/sandbox.rs:254`) inside the
//! *previous* release itself, before it ever opens the migrated schema.
//! That panic (exit 101) was being misread as a schema incompatibility by
//! every caller, when every documented deployment of mcphost (systemd
//! **user** units) already runs the live service unprivileged. When this
//! process's own real uid is 0, [`resolve_run_as`] picks an unprivileged
//! uid/gid for the child -- `--run-as <user>` if given, else the live
//! database file's own owner -- and [`spawn_previous`] drops to it (uid,
//! gid, and supplementary groups all cleared) before exec. Not root:
//! behavior is unchanged. The child's own stderr (bounded to
//! [`STDERR_TAIL_CAP`]) is also captured now and folded into a
//! `previous-up` failure's detail, so a real panic message is visible in
//! the deploy journal instead of only a bare exit status.

use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::db::Db;

/// Shared, growable byte buffer for a child's captured stderr tail --
/// aliased so clippy's `type_complexity` (threshold 200 in this repo's
/// `clippy.toml`) doesn't flag the nested `Arc<Mutex<Vec<u8>>>` at every
/// use site.
type StderrTail = Arc<Mutex<Vec<u8>>>;

/// Which step of the check failed, plus a human-readable detail. `step`
/// values are stable strings (`"copy"`, `"migrate"`, `"spawn"`,
/// `"previous-up"`, `"initialize"`, `"tools_list"`, `"signup"`,
/// `"tools_call"`, `"healthz"`) so a caller (the CLI, or `mcphost-deploy
/// redeploy`) can name the exact failing probe per requirement 2/AC3.
#[derive(Debug)]
pub struct CompatCheckFailure {
    pub step: &'static str,
    pub detail: String,
}

impl std::fmt::Display for CompatCheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.step, self.detail)
    }
}

impl std::error::Error for CompatCheckFailure {}

/// A scratch directory under `std::env::temp_dir()`, removed on drop --
/// same pattern as the integration tests' `TempDataDir`, but living in
/// `src/` because `mcphost migrate --check-compat` runs this in production
/// (via the real CLI binary), not only under `cargo test`.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "mcphost-checkcompat-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the full check. `live_db_path` is the exact sqlite file to copy
/// (opened read-only, never written to); `previous_binary` is the path to
/// the previous release's `mcphost` executable; `run_as` is `--run-as
/// <user>` from the CLI (`None` lets this process pick a default -- see
/// [`resolve_run_as`]). `run_as` is only consulted when this process's own
/// real uid is 0; otherwise it is ignored (the child already runs under
/// whatever unprivileged identity the checker itself does).
pub async fn run(
    live_db_path: &Path,
    previous_binary: &Path,
    run_as: Option<&str>,
) -> Result<(), CompatCheckFailure> {
    fail_if_missing(previous_binary)?;

    let scratch = ScratchDir::new().map_err(|e| CompatCheckFailure {
        step: "copy",
        detail: format!("cannot create scratch dir: {e}"),
    })?;
    let copy_db_path = scratch.0.join("mcphost.db");

    copy_live_db(live_db_path, &copy_db_path)?;

    // Apply this (new) release's pending migrations to the copy only -- the
    // same idempotent migrate_sync() serve/migrate already run, just
    // pointed at the scratch copy.
    {
        let db = Db::open(&scratch.0).map_err(|e| CompatCheckFailure {
            step: "migrate",
            detail: format!("applying pending migrations to the copy failed: {e}"),
        })?;
        db.migrate().await.map_err(|e| CompatCheckFailure {
            step: "migrate",
            detail: format!("applying pending migrations to the copy failed: {e}"),
        })?;
    }

    // Resolved against the live database (already proven to exist by
    // `copy_live_db` above), not the scratch copy -- the copy's ownership
    // is this process's own (it just created it), which tells us nothing
    // about which unprivileged user the real service runs as.
    let resolved_run_as = resolve_run_as(run_as, live_db_path)?;

    let bind_plan = choose_bind_plan()?;
    let base_url = bind_plan.base_url();
    let port = bind_plan.port();
    let token = generate_compat_token();

    let mut spawned =
        spawn_previous(previous_binary, &scratch.0, bind_plan, &token, resolved_run_as)?;
    tracing::info!(
        pid = ?spawned.child.id(),
        port,
        run_as_uid = resolved_run_as.map(|u| u.uid),
        "check-compat: spawned previous release"
    );
    let result = probe_previous(&base_url, port, &token, &mut spawned).await;
    stop(&mut spawned.child).await;

    result
}

fn fail_if_missing(bin: &Path) -> Result<(), CompatCheckFailure> {
    if !bin.is_file() {
        return Err(CompatCheckFailure {
            step: "spawn",
            detail: format!("previous binary not found: {}", bin.display()),
        });
    }
    Ok(())
}

/// The unprivileged uid/gid [`spawn_previous`]'s child should drop to
/// before exec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunAsUser {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

/// Resolve `--run-as <user>` (or, lacking that, the live database's own
/// file owner) to a uid/gid pair -- called once from [`run`], before any
/// child is ever spawned, so a bad `--run-as` value or an unreadable data
/// dir fails fast at a named step (`"spawn"`) rather than silently falling
/// through to `spawn_previous` execing the child as root anyway.
fn resolve_run_as(
    explicit_user: Option<&str>,
    live_db_path: &Path,
) -> Result<Option<RunAsUser>, CompatCheckFailure> {
    // SAFETY: getuid() takes no arguments and cannot fail.
    let real_uid = unsafe { libc::getuid() };
    if real_uid != 0 {
        // Not root: `spawn_previous`'s child already runs under this
        // process's own (already unprivileged) identity -- nothing to
        // drop, exactly as before this fix. `explicit_user` is ignored
        // rather than erroring on an unresolvable name, since a config
        // that always passes `--run-as` must not become a hard failure
        // the one time the checker itself happens to run unprivileged.
        return Ok(None);
    }
    let user = match explicit_user {
        Some(name) => lookup_user(name)?,
        None => owner_of(live_db_path)?,
    };
    Ok(decide_run_as(real_uid, Some(user)))
}

/// Pure decision: does this real uid need the previous release's child
/// dropped to `run_as`? Factored out of [`resolve_run_as`] as a seam --
/// `compatfix` unit tests drive this directly with a synthetic `real_uid`
/// (tests don't run as root, so they can't exercise `resolve_run_as`'s own
/// `libc::getuid()` call end to end): real_uid 0 with a configured
/// `run_as` carries that user's uid/gid through unchanged; any non-zero
/// real_uid clears it to `None` regardless of what `run_as` was resolved
/// to.
fn decide_run_as(real_uid: u32, run_as: Option<RunAsUser>) -> Option<RunAsUser> {
    if real_uid == 0 { run_as } else { None }
}

/// Default when `--run-as` isn't given and this process is root: the live
/// database file's own owner -- the same unprivileged user every
/// documented deployment of mcphost (systemd **user** units, never system
/// units running as root) already runs the live `serve` process as.
fn owner_of(path: &Path) -> Result<RunAsUser, CompatCheckFailure> {
    let meta = std::fs::metadata(path).map_err(|e| CompatCheckFailure {
        step: "spawn",
        detail: format!(
            "running check-compat as root with no --run-as <user>: could not stat {} to \
             default to its owner: {e}",
            path.display()
        ),
    })?;
    Ok(RunAsUser {
        uid: meta.uid(),
        gid: meta.gid(),
    })
}

/// Look up `name` via `getpwnam_r` (the `_r` suffix: thread-safe, unlike
/// `getpwnam`'s static buffer -- this runs inside a multi-threaded tokio
/// runtime).
fn lookup_user(name: &str) -> Result<RunAsUser, CompatCheckFailure> {
    let cname = std::ffi::CString::new(name).map_err(|_| CompatCheckFailure {
        step: "spawn",
        detail: format!("--run-as user name {name:?} contains an interior NUL byte"),
    })?;
    // SAFETY: a zeroed `libc::passwd` is a valid (if meaningless) bit pattern -- it is only ever read after `getpwnam_r` below populates it.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 16 * 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: `cname` and `buf` are valid for the duration of this call; `pwd`/`result` are out-parameters this stack frame owns exclusively.
    let rc = unsafe {
        libc::getpwnam_r(
            cname.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(CompatCheckFailure {
            step: "spawn",
            detail: format!(
                "--run-as {name:?}: getpwnam_r failed: {}",
                std::io::Error::from_raw_os_error(rc)
            ),
        });
    }
    if result.is_null() {
        return Err(CompatCheckFailure {
            step: "spawn",
            detail: format!("--run-as {name:?}: no such user on this box"),
        });
    }
    Ok(RunAsUser {
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
    })
}

/// A single-statement hot copy that works correctly against a live WAL-mode
/// database: `VACUUM INTO` reads the source (opened read-only, so this
/// truly never mutates the live file) and writes a fresh, fully
/// checkpointed copy at `dest`. The destination path is escaped and
/// inlined rather than bound as a parameter -- `VACUUM INTO` takes a
/// filename expression, and inlining with single-quote doubling avoids any
/// ambiguity about parameter support in that position across SQLite
/// versions.
fn copy_live_db(src: &Path, dest: &Path) -> Result<(), CompatCheckFailure> {
    if !src.is_file() {
        return Err(CompatCheckFailure {
            step: "copy",
            detail: format!("live database not found: {}", src.display()),
        });
    }
    let conn = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| CompatCheckFailure {
            step: "copy",
            detail: format!("cannot open live database read-only: {e}"),
        })?;
    let dest_escaped = dest.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{dest_escaped}';"))
        .map_err(|e| CompatCheckFailure {
            step: "copy",
            detail: format!("VACUUM INTO failed: {e}"),
        })?;
    Ok(())
}

/// Bind the loopback listener the previous release will serve on. Kept
/// open (never dropped-then-rebound) all the way through to
/// [`spawn_previous`] handing its fd to the child -- requirement 1's "no
/// window between choosing a port and binding it".
fn bind_loopback_listener() -> std::io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind("127.0.0.1:0")
}

/// A random 128-bit token, hex-encoded, unique per `check_compat` run
/// (requirement 2). Never logged; only compared.
fn generate_compat_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How the previous release will get its listening socket: an inherited
/// fd this process bound itself (the default, race-free path -- goal 2),
/// or a plain address to bind by itself (requirement 4: `$MCPHOST_BIND`
/// set explicitly by the caller keeps working, unchanged, for callers that
/// still pass a port).
enum BindPlan {
    Inherited {
        listener: std::net::TcpListener,
        port: u16,
    },
    Explicit(SocketAddr),
}

impl BindPlan {
    fn port(&self) -> u16 {
        match self {
            BindPlan::Inherited { port, .. } => *port,
            BindPlan::Explicit(addr) => addr.port(),
        }
    }

    fn base_url(&self) -> String {
        match self {
            BindPlan::Inherited { port, .. } => format!("http://127.0.0.1:{port}"),
            BindPlan::Explicit(addr) => format!("http://{addr}"),
        }
    }
}

fn choose_bind_plan() -> Result<BindPlan, CompatCheckFailure> {
    if let Ok(explicit) = std::env::var("MCPHOST_BIND") {
        let addr: SocketAddr = explicit.trim().parse().map_err(|e| CompatCheckFailure {
            step: "spawn",
            detail: format!("$MCPHOST_BIND '{explicit}' is not a valid address: {e}"),
        })?;
        tracing::info!(
            %addr,
            "check-compat: binding previous release by address (MCPHOST_BIND set)"
        );
        return Ok(BindPlan::Explicit(addr));
    }
    let listener = bind_loopback_listener().map_err(|e| CompatCheckFailure {
        step: "spawn",
        detail: format!("could not bind a loopback listener: {e}"),
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| CompatCheckFailure {
            step: "spawn",
            detail: format!("could not read the bound listener's address: {e}"),
        })?
        .port();
    tracing::info!(port, "check-compat: binding previous release via an inherited listener");
    Ok(BindPlan::Inherited { listener, port })
}

/// Fd 3 -- `$LISTEN_FDS_START` in the systemd socket-activation convention.
const LISTEN_FDS_START: std::os::fd::RawFd = 3;

/// Set `$LISTEN_PID` to this (about-to-be-exec'd) process's own pid,
/// without allocating -- called from [`spawn_previous`]'s `pre_exec`
/// closure, which runs strictly between `fork` and `exec` in the freshly
/// forked, single-threaded child (the exact same window systemd's own
/// service manager uses to set this variable for the units it activates).
fn set_listen_pid_to_self() -> std::io::Result<()> {
    // SAFETY: getpid() takes no arguments and cannot fail.
    let pid = unsafe { libc::getpid() };
    let mut digits = [0u8; 10];
    let mut n = pid as u32;
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut value = [0u8; 11]; // up to 10 digits + nul terminator
    let len = digits.len() - i;
    value[..len].copy_from_slice(&digits[i..]);
    // SAFETY: `value` is nul-terminated ASCII digits, `c"LISTEN_PID"` is a static nul-terminated literal; setenv is called once, synchronously, before this process ever execs.
    let rc = unsafe { libc::setenv(c"LISTEN_PID".as_ptr(), value.as_ptr().cast(), 1) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// How much of the previous release's own stderr `check-compat` keeps
/// around to fold into a `previous-up` failure's detail (PRD-mcphost-
/// compat-check-unprivileged requirement 2) -- enough for a Rust panic's
/// full one-line message, bounded so a previous release that floods
/// stderr can't grow this process's memory without limit.
const STDERR_TAIL_CAP: usize = 4096;

/// A spawned previous release, bundled with the tail of its own stderr --
/// [`wait_ready`]/[`probe_previous`] read `stderr_tail` on failure so the
/// real cause (a panic, a permissions error) is visible, not just the
/// bare exit status.
struct SpawnedChild {
    child: Child,
    stderr_tail: StderrTail,
}

/// Drain `stderr` into `tail` as it arrives, keeping only the last
/// [`STDERR_TAIL_CAP`] bytes -- must run continuously for as long as the
/// child is alive, or a previous release that writes enough to fill the
/// pipe buffer would block on its own stderr forever.
fn spawn_stderr_tail_reader(mut stderr: tokio::process::ChildStderr, tail: StderrTail) {
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut locked = tail.lock().unwrap_or_else(|e| e.into_inner());
                    locked.extend_from_slice(&buf[..n]);
                    let len = locked.len();
                    if len > STDERR_TAIL_CAP {
                        locked.drain(0..len - STDERR_TAIL_CAP);
                    }
                }
            }
        }
    });
}

/// Render whatever's currently in `tail` as a trimmed, lossily-decoded
/// string -- stderr is operator-facing text, not guaranteed-UTF8 wire
/// data, so lossless decoding isn't the goal here.
fn stderr_tail_text(tail: &StderrTail) -> String {
    let locked = tail.lock().unwrap_or_else(|e| e.into_inner());
    String::from_utf8_lossy(&locked).trim().to_string()
}

/// Drop `cmd`'s child to `user`'s uid/gid before exec, with supplementary
/// groups cleared -- a no-op when `user` is `None` (this process isn't
/// root; see [`resolve_run_as`]).
fn apply_run_as(cmd: &mut Command, user: Option<RunAsUser>) {
    let Some(user) = user else { return };
    // Order matters and is done by hand here (not via `Command::uid`/`gid`) because those two
    // alone leave root's own supplementary groups (gid 0, plus any wheel/sudo/docker membership)
    // riding along on the child even after its primary uid/gid drops: `setgroups` clears them
    // first, while this process can still call it, then `setgid` then `setuid` last, in that
    // order, since `setuid` is what gives up the ability to change any of the three.
    // SAFETY: this closure runs strictly between fork and exec in the freshly forked, single-threaded child -- the same window `BindPlan::Inherited`'s own `pre_exec` closure below uses.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setgroups(0, std::ptr::null()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setgid(user.gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(user.uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Spawn the previous release's binary against the scratch copy. Per
/// requirement 2's technical considerations: reuse the caller's env
/// contract (`/etc/mcphost/env`, already exported into this process's
/// environment by the deploy tool) with only `MCPHOST_DATA_DIR`,
/// `MCPHOST_COMPAT_TOKEN`, and the bind plan's env overridden. `run_as`
/// (resolved by [`resolve_run_as`]) drops the child to an unprivileged
/// uid/gid first when this process itself is root -- see the module doc
/// comment and PRD-mcphost-compat-check-unprivileged.
fn spawn_previous(
    bin: &Path,
    data_dir: &Path,
    bind_plan: BindPlan,
    token: &str,
    run_as: Option<RunAsUser>,
) -> Result<SpawnedChild, CompatCheckFailure> {
    let mut cmd = Command::new(bin);
    cmd.arg("serve")
        .env("MCPHOST_DATA_DIR", data_dir)
        .env("MCPHOST_COMPAT_TOKEN", token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    match bind_plan {
        BindPlan::Explicit(addr) => {
            cmd.env("MCPHOST_BIND", addr.to_string());
        }
        BindPlan::Inherited { listener, .. } => {
            cmd.env("LISTEN_FDS", "1");
            // SAFETY: this closure runs strictly between fork and exec in the freshly forked, single-threaded child; `dup2` and `setenv` (inside `set_listen_pid_to_self`) are the same primitives systemd's own service manager uses to implement this exact convention. `listener` is moved in so its fd stays open (and thus reserved) across the fork.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::dup2(listener.as_raw_fd(), LISTEN_FDS_START) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    set_listen_pid_to_self()?;
                    Ok(())
                });
            }
        }
    }

    // Registered last so, if this process is root, the privilege drop
    // happens as the final step before exec -- after the inherited-fd
    // dup2/env setup above, neither of which needs any privilege to run.
    apply_run_as(&mut cmd, run_as);

    let mut child = cmd.spawn().map_err(|e| CompatCheckFailure {
        step: "spawn",
        detail: format!("failed to spawn {}: {e}", bin.display()),
    })?;

    let stderr_tail = Arc::new(Mutex::new(Vec::new()));
    if let Some(stderr) = child.stderr.take() {
        spawn_stderr_tail_reader(stderr, Arc::clone(&stderr_tail));
    }

    Ok(SpawnedChild { child, stderr_tail })
}

async fn stop(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `port` (for the foreign-server log line) and `stderr_tail` bundled so
/// [`wait_ready`] stays under this repo's `too-many-arguments-threshold =
/// 5`, same reason [`RpcRequest`] exists.
struct WaitReadyContext<'a> {
    port: u16,
    stderr_tail: &'a StderrTail,
}

/// Poll `{base_url}/healthz` until it answers with our own
/// `X-Mcphost-Compat-Token` (requirement 2), the child exits (requirement
/// 3 -- checked first, every iteration, so a previous release that can
/// never come up fails in one poll interval instead of after
/// `READY_TIMEOUT`), or `READY_TIMEOUT` elapses.
async fn wait_ready(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    child: &mut Child,
    ctx: WaitReadyContext<'_>,
) -> Result<(), CompatCheckFailure> {
    let WaitReadyContext { port, stderr_tail } = ctx;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| CompatCheckFailure {
            step: "previous-up",
            detail: format!("failed to poll the previous release's process: {e}"),
        })? {
            // Requirement 2: give the background stderr reader
            // (`spawn_stderr_tail_reader`) a moment to drain whatever the
            // child wrote before it exited -- the pipe's write end closed
            // the instant the child did, but this process's own tokio
            // task that reads it may not have been polled yet.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let tail = stderr_tail_text(stderr_tail);
            tracing::error!(
                exit_status = %status,
                stderr_tail = %tail,
                "previous release exited before healthz became ready"
            );
            let detail = if tail.is_empty() {
                format!("previous release exited before healthz became ready ({status})")
            } else {
                format!(
                    "previous release exited before healthz became ready ({status}); stderr: {tail}"
                )
            };
            return Err(CompatCheckFailure {
                step: "previous-up",
                detail,
            });
        }

        if let Ok(resp) = client.get(format!("{base_url}/healthz")).send().await
            && resp.status().is_success()
        {
            let has_our_token = resp
                .headers()
                .get("X-Mcphost-Compat-Token")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v == token);
            if has_our_token {
                return Ok(());
            }
            tracing::warn!(
                "healthz answered without our token (foreign server on port {port})"
            );
        }

        if Instant::now() >= deadline {
            let tail = stderr_tail_text(stderr_tail);
            let detail = if tail.is_empty() {
                "previous release did not answer /healthz with our token within 10s".to_string()
            } else {
                format!(
                    "previous release did not answer /healthz with our token within 10s; \
                     stderr: {tail}"
                )
            };
            return Err(CompatCheckFailure {
                step: "previous-up",
                detail,
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Test-only entry points into this module's normally-private pieces --
/// PRD-mcphost-checkcompat-port-race's stress/foreign-server/inherited-fd
/// tests need to drive `wait_ready`/`spawn_previous` directly rather than
/// only through the full `mcphost migrate --check-compat` subprocess (see
/// `tests/checkcompat_race_ac*.rs`). Gated exactly like
/// `billing::FakeBillingClient` and friends.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    pub fn bind_loopback_listener() -> std::io::Result<std::net::TcpListener> {
        super::bind_loopback_listener()
    }

    pub fn generate_compat_token() -> String {
        super::generate_compat_token()
    }

    /// Spawn `bin serve` against `data_dir`, handing it `listener` as an
    /// inherited socket and `token` as `$MCPHOST_COMPAT_TOKEN` -- the same
    /// path `run()` takes when the caller hasn't set `$MCPHOST_BIND`. No
    /// `--run-as` (`None`) -- matches every existing caller of this
    /// wrapper, none of which care about the unprivileged-child behavior.
    pub fn spawn_previous_inherited(
        bin: &Path,
        data_dir: &Path,
        listener: std::net::TcpListener,
        token: &str,
    ) -> Result<Child, CompatCheckFailure> {
        Ok(spawn_previous_inherited_with_tail(bin, data_dir, listener, token)?.0)
    }

    /// Same as [`spawn_previous_inherited`], but also hands back the
    /// child's own stderr tail buffer -- PRD-mcphost-compat-check-
    /// unprivileged's `compatfix` tests need to assert on it directly.
    pub fn spawn_previous_inherited_with_tail(
        bin: &Path,
        data_dir: &Path,
        listener: std::net::TcpListener,
        token: &str,
    ) -> Result<(Child, StderrTail), CompatCheckFailure> {
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let spawned = super::spawn_previous(
            bin,
            data_dir,
            BindPlan::Inherited { listener, port },
            token,
            None,
        )?;
        Ok((spawned.child, spawned.stderr_tail))
    }

    pub async fn wait_ready(
        base_url: &str,
        port: u16,
        token: &str,
        child: &mut Child,
    ) -> Result<(), CompatCheckFailure> {
        let tail = Arc::new(Mutex::new(Vec::new()));
        wait_ready_with_tail(base_url, port, token, child, &tail).await
    }

    /// Same as [`wait_ready`], but reads the stderr tail from `tail`
    /// (as filled by a [`spawn_previous_inherited_with_tail`] child)
    /// instead of an always-empty throwaway buffer.
    pub async fn wait_ready_with_tail(
        base_url: &str,
        port: u16,
        token: &str,
        child: &mut Child,
        tail: &StderrTail,
    ) -> Result<(), CompatCheckFailure> {
        let client = reqwest::Client::new();
        super::wait_ready(
            &client,
            base_url,
            token,
            child,
            super::WaitReadyContext {
                port,
                stderr_tail: tail,
            },
        )
        .await
    }
}

/// The stateless SEP-2575 `_meta` block every non-`initialize` request
/// needs, matching `tests/common/mod.rs`'s client.
fn with_client_meta(mut params: Value) -> Value {
    if let Some(obj) = params.as_object_mut() {
        obj.insert(
            "_meta".to_string(),
            json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
    }
    params
}

/// One JSON-RPC-over-streamable-HTTP request, bundled so [`rpc_call`] stays
/// under this repo's `too-many-arguments-threshold = 5`.
struct RpcRequest<'a> {
    method: &'a str,
    params: Value,
    id: u64,
    bearer: Option<&'a str>,
}

async fn rpc_call(
    client: &reqwest::Client,
    base_url: &str,
    req: RpcRequest<'_>,
) -> Result<Value, String> {
    let RpcRequest {
        method,
        params,
        id,
        bearer,
    } = req;
    let body_params = if method == "initialize" {
        params
    } else {
        with_client_meta(params)
    };
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": body_params});

    let mut http_req = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&body);
    if method != "initialize" {
        http_req = http_req.header("Mcp-Method", method);
        if method == "tools/call"
            && let Some(name) = tool_call_name(&body)
        {
            http_req = http_req.header("Mcp-Name", name);
        }
    }
    if let Some(key) = bearer {
        http_req = http_req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = http_req
        .send()
        .await
        .map_err(|e| format!("{method} request failed: {e}"))?;
    let status = resp.status();
    // PRD-mcphost-one-next-tool requirement 3: a call that binds this
    // connection (`signup`) now emits notifications/tools/list_changed
    // before its own result, which upgrades that one response from
    // `application/json` to `text/event-stream` (`rmcp`'s own
    // `StreamableHttpServerConfig::json_response` fallback) -- the real
    // JSON-RPC message is always the last one on the wire.
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = if content_type.starts_with("text/event-stream") {
        let text = resp
            .text()
            .await
            .map_err(|e| format!("{method} response ({status}) did not read as text: {e}"))?;
        text.split("\n\n")
            .filter_map(|block| {
                block.lines().find_map(|line| {
                    line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:"))
                })
            })
            .filter(|data| !data.is_empty())
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .collect::<Vec<Value>>()
            .pop()
            .ok_or_else(|| format!("{method} SSE response ({status}) carried no JSON-RPC message"))?
    } else {
        resp.json()
            .await
            .map_err(|e| format!("{method} response ({status}) did not parse as JSON: {e}"))?
    };
    if let Some(error) = parsed.get("error") {
        return Err(format!("{method} returned a JSON-RPC error: {error}"));
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

fn tool_call_name(body: &Value) -> Option<String> {
    body.get("params")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `CallToolResult::structured` puts the value in `structuredContent`;
/// fall back to the first text content block, matching the test harness's
/// `extract_structured`.
fn extract_structured(call_result: &Value) -> Value {
    if let Some(sc) = call_result.get("structuredContent") {
        return sc.clone();
    }
    if let Some(content) = call_result.get("content").and_then(Value::as_array)
        && let Some(first) = content.first()
        && let Some(text) = first.get("text").and_then(Value::as_str)
    {
        return serde_json::from_str(text).unwrap_or(Value::Null);
    }
    Value::Null
}

async fn probe_previous(
    base_url: &str,
    port: u16,
    token: &str,
    spawned: &mut SpawnedChild,
) -> Result<(), CompatCheckFailure> {
    let client = reqwest::Client::new();
    wait_ready(
        &client,
        base_url,
        token,
        &mut spawned.child,
        WaitReadyContext {
            port,
            stderr_tail: &spawned.stderr_tail,
        },
    )
    .await?;
    tracing::info!(port, "check-compat: previous release answered healthz with our token");

    rpc_call(
        &client,
        base_url,
        RpcRequest {
            method: "initialize",
            params: json!({
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "mcphost-check-compat", "version": env!("CARGO_PKG_VERSION")}
            }),
            id: 1,
            bearer: None,
        },
    )
    .await
    .map_err(|detail| CompatCheckFailure {
        step: "initialize",
        detail,
    })?;

    rpc_call(
        &client,
        base_url,
        RpcRequest {
            method: "tools/list",
            params: json!({}),
            id: 2,
            bearer: None,
        },
    )
    .await
    .map_err(|detail| CompatCheckFailure {
        step: "tools_list",
        detail,
    })?;

    // Bootstrap a throwaway tenant on the scratch copy so there is a key to
    // reach a control-plane tool with -- discarded with the scratch
    // directory when the check ends.
    let signup_result = rpc_call(
        &client,
        base_url,
        RpcRequest {
            method: "tools/call",
            params: json!({"name": "signup", "arguments": {"name": "check-compat"}}),
            id: 3,
            bearer: None,
        },
    )
    .await
    .map_err(|detail| CompatCheckFailure {
        step: "signup",
        detail,
    })?;
    let structured = extract_structured(&signup_result);
    let key = structured
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| CompatCheckFailure {
            step: "signup",
            detail: "signup response had no `key` field".to_string(),
        })?
        .to_string();

    // requirement 2's "one tools/call of a control-plane tool":
    // `host.whoami` is read-only and side-effect free.
    rpc_call(
        &client,
        base_url,
        RpcRequest {
            method: "tools/call",
            params: json!({"name": "host.whoami", "arguments": {}}),
            id: 4,
            bearer: Some(&key),
        },
    )
    .await
    .map_err(|detail| CompatCheckFailure {
        step: "tools_call",
        detail,
    })?;

    let health = client
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .map_err(|e| CompatCheckFailure {
            step: "healthz",
            detail: format!("GET /healthz failed: {e}"),
        })?;
    if !health.status().is_success() {
        return Err(CompatCheckFailure {
            step: "healthz",
            detail: format!("GET /healthz returned {}", health.status()),
        });
    }

    Ok(())
}

/// PRD-mcphost-compat-check-unprivileged (b)/(c): `decide_run_as` is the
/// seam -- these tests can't call the real `libc::getuid()` and get 0
/// (they don't run as root), so they drive the pure decision function
/// directly with a synthetic `real_uid` instead.
#[cfg(test)]
mod compatfix_run_as_tests {
    use super::{RunAsUser, decide_run_as};

    #[test]
    fn compatfix_root_with_configured_user_carries_its_uid_gid() {
        let user = RunAsUser { uid: 65534, gid: 65534 };
        let decided = decide_run_as(0, Some(user));
        assert_eq!(decided, Some(user), "real uid 0 must carry the configured run_as through");
    }

    #[test]
    fn compatfix_non_root_never_sets_uid_gid() {
        let user = RunAsUser { uid: 65534, gid: 65534 };
        let decided = decide_run_as(1000, Some(user));
        assert_eq!(
            decided, None,
            "a non-root checker must never apply a uid/gid override, even if one was resolved"
        );
    }

    #[test]
    fn compatfix_root_with_no_configured_user_stays_none() {
        // Shouldn't happen in practice (resolve_run_as always resolves a
        // default before calling this), but the seam itself must not
        // invent a uid/gid out of thin air.
        assert_eq!(decide_run_as(0, None), None);
    }
}
