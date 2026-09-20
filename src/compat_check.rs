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

use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::process::{Child, Command};

use crate::db::Db;

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
/// the previous release's `mcphost` executable.
pub async fn run(live_db_path: &Path, previous_binary: &Path) -> Result<(), CompatCheckFailure> {
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

    let bind_plan = choose_bind_plan()?;
    let base_url = bind_plan.base_url();
    let port = bind_plan.port();
    let token = generate_compat_token();

    let mut child = spawn_previous(previous_binary, &scratch.0, bind_plan, &token)?;
    tracing::info!(
        pid = ?child.id(),
        port,
        "check-compat: spawned previous release"
    );
    let result = probe_previous(&base_url, port, &token, &mut child).await;
    stop(&mut child).await;

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
    // SAFETY: `value` is nul-terminated ASCII digits, `c"LISTEN_PID"` is a
    // static nul-terminated literal; setenv is called once, synchronously,
    // before this process ever execs.
    let rc = unsafe { libc::setenv(c"LISTEN_PID".as_ptr(), value.as_ptr().cast(), 1) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Spawn the previous release's binary against the scratch copy. Per
/// requirement 2's technical considerations: reuse the caller's env
/// contract (`/etc/mcphost/env`, already exported into this process's
/// environment by the deploy tool) with only `MCPHOST_DATA_DIR`,
/// `MCPHOST_COMPAT_TOKEN`, and the bind plan's env overridden.
fn spawn_previous(
    bin: &Path,
    data_dir: &Path,
    bind_plan: BindPlan,
    token: &str,
) -> Result<Child, CompatCheckFailure> {
    let mut cmd = Command::new(bin);
    cmd.arg("serve")
        .env("MCPHOST_DATA_DIR", data_dir)
        .env("MCPHOST_COMPAT_TOKEN", token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    match bind_plan {
        BindPlan::Explicit(addr) => {
            cmd.env("MCPHOST_BIND", addr.to_string());
        }
        BindPlan::Inherited { listener, .. } => {
            cmd.env("LISTEN_FDS", "1");
            // SAFETY: this closure runs strictly between fork and exec in
            // the freshly forked, single-threaded child; `dup2` and
            // `setenv` (inside `set_listen_pid_to_self`) are the same
            // primitives systemd's own service manager uses to implement
            // this exact convention. `listener` is moved in so its fd
            // stays open (and thus reserved) across the fork.
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

    cmd.spawn().map_err(|e| CompatCheckFailure {
        step: "spawn",
        detail: format!("failed to spawn {}: {e}", bin.display()),
    })
}

async fn stop(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Poll `{base_url}/healthz` until it answers with our own
/// `X-Mcphost-Compat-Token` (requirement 2), the child exits (requirement
/// 3 -- checked first, every iteration, so a previous release that can
/// never come up fails in one poll interval instead of after
/// `READY_TIMEOUT`), or `READY_TIMEOUT` elapses.
async fn wait_ready(
    client: &reqwest::Client,
    base_url: &str,
    port: u16,
    token: &str,
    child: &mut Child,
) -> Result<(), CompatCheckFailure> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| CompatCheckFailure {
            step: "previous-up",
            detail: format!("failed to poll the previous release's process: {e}"),
        })? {
            tracing::error!(
                exit_status = %status,
                "previous release exited before healthz became ready"
            );
            return Err(CompatCheckFailure {
                step: "previous-up",
                detail: format!(
                    "previous release exited before healthz became ready ({status})"
                ),
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
            return Err(CompatCheckFailure {
                step: "previous-up",
                detail: "previous release did not answer /healthz with our token within 10s"
                    .into(),
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
    /// path `run()` takes when the caller hasn't set `$MCPHOST_BIND`.
    pub fn spawn_previous_inherited(
        bin: &Path,
        data_dir: &Path,
        listener: std::net::TcpListener,
        token: &str,
    ) -> Result<Child, CompatCheckFailure> {
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        super::spawn_previous(bin, data_dir, BindPlan::Inherited { listener, port }, token)
    }

    pub async fn wait_ready(
        base_url: &str,
        port: u16,
        token: &str,
        child: &mut Child,
    ) -> Result<(), CompatCheckFailure> {
        let client = reqwest::Client::new();
        super::wait_ready(&client, base_url, port, token, child).await
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
    let parsed: Value = resp
        .json()
        .await
        .map_err(|e| format!("{method} response ({status}) did not parse as JSON: {e}"))?;
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
    child: &mut Child,
) -> Result<(), CompatCheckFailure> {
    let client = reqwest::Client::new();
    wait_ready(&client, base_url, port, token, child).await?;
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
