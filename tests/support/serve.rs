//! PRD-mcphost-child-process-test-harness: the one place a test spawns a
//! live child process. [`ServeChild`] owns spawn -> ready -> diagnostics ->
//! kill for `mcphost serve`, so no test hand-rolls a `Command` chain, a
//! readiness deadline, a `ChildGuard`, or a blocking read on a live child's
//! pipe (gate hangs 2026-10-09, runs 618 + 619: a `read_to_string` on a live
//! child's stderr sat before the kill that `ChildGuard::drop` would have
//! issued after the panic that follows the read).
//!
//! Not a crate module: pulled in per-file with `#[path = "support/serve.rs"]
//! mod serve;`, same convention as `tests/support/host.rs`. The pub fn names
//! in this file ARE the allowed-call set `scripts/child-proc-lint.py`
//! enforces; the script reads them from here, so adding a helper needs no
//! edit to the script.

#![allow(dead_code)]

use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child as AsyncChild, ChildStdout};
use tokio::time::error::Elapsed;

/// Per-probe HTTP timeout inside the readiness loop. A server that accepts
/// the connection but never answers must not stall the loop past its
/// deadline.
const READY_PROBE_TIMEOUT_MS: u64 = 2_000;
/// Pause between readiness probes.
const READY_POLL_MS: u64 = 20;

/// What a `mcphost serve` child is started with. `data_dir` is required;
/// everything else is optional. `MCPHOST_BIND` is always set by
/// [`ServeChild::spawn`] to a free loopback port.
#[derive(Clone, Default)]
pub struct ServeEnv {
    pub data_dir: PathBuf,
    pub admin_key: Option<String>,
    pub secret_key: Option<String>,
    /// Further `(name, value)` env pairs, e.g. `("MCPHOST_LOG_LEVEL", "warn")`.
    pub extra_env: Vec<(String, String)>,
    /// Capture the child's stdout (where `tracing` writes by default) into a
    /// buffer read by [`ServeChild::stdout_log`]. Off: stdout is null.
    pub drain_stdout: bool,
}

/// A live child process plus everything needed to diagnose and reap it.
/// Dropping it kills and waits.
pub struct ServeChild {
    child: Child,
    stderr: Option<ChildStderr>,
    port: u16,
    data_dir: PathBuf,
    stdout_buf: Arc<Mutex<String>>,
    drained: bool,
    ready_budget: Option<Duration>,
}

fn free_loopback_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

impl ServeChild {
    /// Spawn `mcphost serve` on a free loopback port with `env`.
    pub fn spawn(env: ServeEnv) -> ServeChild {
        let port = free_loopback_port();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
        cmd.arg("serve")
            .env("MCPHOST_DATA_DIR", &env.data_dir)
            .env("MCPHOST_BIND", format!("127.0.0.1:{port}"));
        if let Some(k) = &env.admin_key {
            cmd.env("MCPHOST_ADMIN_KEY", k);
        }
        if let Some(k) = &env.secret_key {
            cmd.env("MCPHOST_SECRET_KEY", k);
        }
        for (k, v) in &env.extra_env {
            cmd.env(k, v);
        }
        Self::start(cmd, port, env.data_dir, env.drain_stdout)
    }

    /// Spawn an arbitrary command as the "server" and treat `port` as the
    /// port its `/healthz` should answer on. For regression tests of the
    /// failure path (a stand-in that never answers) and for non-serve
    /// children whose stderr a test wants diagnosed safely.
    pub fn spawn_command(cmd: Command, port: u16) -> ServeChild {
        Self::start(cmd, port, PathBuf::new(), false)
    }

    fn start(mut cmd: Command, port: u16, data_dir: PathBuf, drain: bool) -> ServeChild {
        cmd.stdin(Stdio::null()).stderr(Stdio::piped());
        cmd.stdout(if drain { Stdio::piped() } else { Stdio::null() });
        let mut child = cmd.spawn().expect("spawn child process");
        let stderr = child.stderr.take();
        let stdout_buf = Arc::new(Mutex::new(String::new()));
        if let Some(mut out) = child.stdout.take() {
            // Drain for the child's whole lifetime: a full pipe would block
            // the server in `write`. The thread ends at EOF, i.e. when the
            // child is killed.
            let buf = stdout_buf.clone();
            std::thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = out.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buf.lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            });
        }
        ServeChild {
            child,
            stderr,
            port,
            data_dir,
            stdout_buf,
            drained: drain,
            ready_budget: None,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// OS process id of the child.
    pub fn child_id(&self) -> u32 {
        self.child.id()
    }

    /// Send the child a real SIGTERM.
    pub fn terminate(&self) {
        // SAFETY: kill(2) with a pid this struct owns and a valid signal.
        let rc = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        assert_eq!(rc, 0, "kill(SIGTERM) failed");
    }

    /// Wait up to `timeout` for the child to exit; `None` if it is still
    /// running at the deadline.
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The readiness budget the last [`ServeChild::wait_ready`] used (`None`
    /// before the first call).
    pub fn ready_budget(&self) -> Option<Duration> {
        self.ready_budget
    }

    /// Everything the child has written to stdout so far (empty unless the
    /// child was spawned with `drain_stdout: true`).
    pub fn stdout_log(&self) -> String {
        self.stdout_buf.lock().unwrap().clone()
    }

    /// Kill and reap the child FIRST, then read its stderr (EOF is
    /// guaranteed once it has exited). Never blocks on a live child.
    /// Returns an empty string on a second call (the pipe is consumed).
    pub fn diagnostics(&mut self) -> String {
        crate::common::kill_then_read_stderr(&mut self.child, self.stderr.take())
    }

    /// Poll `/healthz` until it answers, the child exits, or
    /// `serve_ready_timeout()` elapses. On failure: kill the child, then
    /// panic with its stderr (`stderr:` in the message), so the test fails
    /// within the deadline plus a probe timeout instead of blocking.
    pub async fn wait_ready(&mut self) {
        let budget = crate::common::serve_ready_timeout();
        self.ready_budget = Some(budget);
        let deadline = Instant::now() + budget;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(READY_PROBE_TIMEOUT_MS))
            .build()
            .expect("build readiness client");
        let url = format!("{}/healthz", self.base_url());
        loop {
            if http.get(&url).send().await.is_ok() {
                return;
            }
            let exited = matches!(self.child.try_wait(), Ok(Some(_)));
            if exited || Instant::now() > deadline {
                let why = if exited { "exited before becoming healthy" } else { "did not become healthy in time" };
                let stdout = if self.drained {
                    format!("\nstdout:\n{}", self.stdout_log())
                } else {
                    String::new()
                };
                let buf = self.diagnostics();
                panic!("mcphost serve {why} (budget {budget:?}); stderr:\n{buf}{stdout}");
            }
            tokio::time::sleep(Duration::from_millis(READY_POLL_MS)).await;
        }
    }
}

impl Drop for ServeChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A long-running child whose stdout is read line by line while it is still
/// alive (`mcphost push --watch`), the streaming counterpart of
/// [`ServeChild`], whose diagnostics kill before they read. The stdout pipe
/// never leaves this struct and every wait is bounded by the caller's
/// deadline. Dropping it kills the child (`kill_on_drop`).
pub struct ServeLines {
    _child: AsyncChild,
    lines: Lines<BufReader<ChildStdout>>,
}

impl ServeLines {
    /// Start `cmd` with stdout piped and kill-on-drop; stdin, stderr and the
    /// rest of its configuration are the caller's.
    pub fn spawn_lines(mut cmd: tokio::process::Command) -> ServeLines {
        cmd.stdout(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn().expect("spawn child process");
        let out = child.stdout.take().expect("stdout was piped");
        ServeLines {
            _child: child,
            lines: BufReader::new(out).lines(),
        }
    }

    /// The child's next stdout line: `Ok(Some(line))`, `Ok(None)` at EOF, or
    /// `Err(Elapsed)` when nothing arrived within `within`.
    pub async fn next_line(&mut self, within: Duration) -> Result<Option<String>, Elapsed> {
        tokio::time::timeout(within, self.lines.next_line())
            .await
            .map(|line| line.expect("read child stdout"))
    }
}
