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

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::process::{Child, Command};

use crate::db::Db;

/// Which step of the check failed, plus a human-readable detail. `step`
/// values are stable strings (`"copy"`, `"migrate"`, `"spawn"`,
/// `"initialize"`, `"tools_list"`, `"signup"`, `"tools_call"`, `"healthz"`)
/// so a caller (the CLI, or `mcphost-deploy redeploy`) can name the exact
/// failing probe per requirement 2/AC3.
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

    let token = generate_compat_token();

    // Requirement 4 / AC6: a caller that sets `MCPHOST_BIND` in *this*
    // process's own environment before invoking `check_compat` wants the
    // old bind-by-address path (the previous binary binds itself), not the
    // new inherited-listener path -- still with the token enforced.
    let (mut child, base_url, bind_desc) = match std::env::var("MCPHOST_BIND") {
        Ok(addr) => {
            let child = spawn_previous_by_address(previous_binary, &scratch.0, &addr, &token)?;
            (child, format!("http://{addr}"), format!("bound by address {addr} (MCPHOST_BIND)"))
        }
        Err(_) => {
            let listener = bind_loopback_listener().map_err(|e| CompatCheckFailure {
                step: "spawn",
                detail: format!("could not bind a loopback listener: {e}"),
            })?;
            let port = listener
                .local_addr()
                .map_err(|e| CompatCheckFailure {
                    step: "spawn",
                    detail: format!("could not read bound listener's address: {e}"),
                })?
                .port();
            let child = spawn_previous_inherited(previous_binary, &scratch.0, &listener, &token)?;
            // The listener's underlying socket stays alive via the child's
            // own dup'd fd 3 (see `spawn_previous_inherited`'s pre_exec) --
            // dropping our copy here does not close the port, and there
            // was no window between bind and hand-off for anyone else to
            // race into (requirement 1).
            drop(listener);
            (
                child,
                format!("http://127.0.0.1:{port}"),
                format!("inherited listener fd (LISTEN_FDS) on port {port}"),
            )
        }
    };

    // Requirement 6 / P2: log the chosen bind mode, child pid, at info.
    tracing::info!(
        pid = child.id(),
        bind = %bind_desc,
        "compat_check: spawned previous binary"
    );

    let result = probe_previous(&base_url, &token, &mut child).await;
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

/// P0 requirement 1: replaces the old `free_loopback_port` (bind, read the
/// port, drop the listener, hope the child binds it before someone else
/// does). This binds `127.0.0.1:0` and hands the still-open listener back
/// to the caller, which keeps it alive until the child has its own
/// reference (see `spawn_previous_inherited`) -- there is never a moment
/// where the chosen port is unbound.
fn bind_loopback_listener() -> std::io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind("127.0.0.1:0")
}

/// A random 128-bit per-invocation token (requirement 2): the readiness
/// probe only trusts a `/healthz` response carrying this exact value back,
/// so a foreign server that happens to be listening on the same port (the
/// original race) can never be mistaken for the child this call spawned.
fn generate_compat_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Spawn the previous release's binary with the pre-bound listener handed
/// over as an inherited socket, using the systemd socket-activation
/// convention (`LISTEN_FDS=1` / `LISTEN_PID=<child>`, fd 3 =
/// `SD_LISTEN_FDS_START`) that `mcphost serve` now honors
/// (`src/http.rs::inherited_listener`). Per requirement 2's technical
/// considerations: reuse the caller's env contract (`/etc/mcphost/env`,
/// already exported into this process's environment by the deploy tool)
/// with only `MCPHOST_DATA_DIR` overridden, plus the new
/// `MCPHOST_COMPAT_TOKEN`. `MCPHOST_BIND` is explicitly removed so a
/// leftover value from this process's own env can never make the child
/// bind by address instead of using the inherited fd.
fn spawn_previous_inherited(
    bin: &Path,
    data_dir: &Path,
    listener: &std::net::TcpListener,
    token: &str,
) -> Result<Child, CompatCheckFailure> {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt;

    const SD_LISTEN_FDS_START: i32 = 3;
    let raw_fd = listener.as_raw_fd();

    let mut std_cmd = std::process::Command::new(bin);
    std_cmd
        .arg("serve")
        .env("MCPHOST_DATA_DIR", data_dir)
        .env("MCPHOST_COMPAT_TOKEN", token)
        // `LISTEN_FDS` is set here, in the PARENT before fork -- never
        // inside `pre_exec` (see the SAFETY note below for why that
        // matters). No `LISTEN_PID`: matching it would need the child's
        // own post-fork pid, and computing/storing that from inside
        // `pre_exec` runs into the exact hazard this comment documents,
        // for no real benefit here (the direct, sole consumer of this fd
        // is `serve` itself -- it never re-execs a grandchild that could
        // mistake the same fd for its own, which is the scenario
        // `LISTEN_PID` actually guards against in systemd proper).
        .env("LISTEN_FDS", "1")
        .env_remove("MCPHOST_BIND")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Fork hazard found live during this PRD's own stress test (AC5): every
    // spawned child hung forever, 0% CPU, still showing the PARENT's argv
    // in `/proc/<pid>/cmdline` -- it never reached `execve`. `fork()` from
    // a multithreaded process (this one -- tokio runs a worker pool) only
    // carries the CALLING thread into the child; any lock some OTHER
    // thread held at fork time (critically Rust's own internal env lock)
    // is inherited already-held and never released, because the thread
    // that would release it does not exist in the child.
    // `std::env::set_var` takes exactly that lock, so a `pre_exec` closure
    // that calls it deadlocks the child before `exec` runs. This closure
    // now does ONLY `dup2`, a raw libc syscall that touches no lock.
    // SAFETY: `dup2` is async-signal-safe / fork-safe by design (no allocation, no locks); it clears close-on-exec on the target descriptor regardless of the source's flag (POSIX dup2 semantics), so fd 3 survives the exec below.
    unsafe {
        std_cmd.pre_exec(move || {
            if raw_fd != SD_LISTEN_FDS_START {
                let rc = libc::dup2(raw_fd, SD_LISTEN_FDS_START);
                if rc < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    let mut tokio_cmd = Command::from(std_cmd);
    tokio_cmd.kill_on_drop(true);
    tokio_cmd.spawn().map_err(|e| CompatCheckFailure {
        step: "spawn",
        detail: format!("failed to spawn {}: {e}", bin.display()),
    })
}

/// Requirement 4 / AC6: the old bind-by-address path, kept for a caller
/// that sets `MCPHOST_BIND` explicitly -- still enforces the token.
fn spawn_previous_by_address(
    bin: &Path,
    data_dir: &Path,
    bind_addr: &str,
    token: &str,
) -> Result<Child, CompatCheckFailure> {
    Command::new(bin)
        .arg("serve")
        .env("MCPHOST_DATA_DIR", data_dir)
        .env("MCPHOST_BIND", bind_addr)
        .env("MCPHOST_COMPAT_TOKEN", token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| CompatCheckFailure {
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
const COMPAT_TOKEN_HEADER: &str = "X-Mcphost-Compat-Token";

/// Requirement 2 (token match) + requirement 3 (short-circuit on early
/// exit). `child` is polled with `try_wait()` every iteration so a binary
/// that exits immediately (`/bin/false`) fails in one `POLL_INTERVAL`, not
/// after the full 10s timeout, and the returned detail names the exit
/// status. A 200 with no/mismatched token is logged and treated as
/// not-ready rather than accepted -- that is the whole fix for the
/// original race (a parallel test's real server answering for this one).
async fn wait_ready(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    child: &mut Child,
) -> Result<(), CompatCheckFailure> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(CompatCheckFailure {
                step: "spawn",
                detail: format!("previous binary exited before becoming ready: {status}"),
            });
        }
        if let Ok(resp) = client.get(format!("{base_url}/healthz")).send().await
            && resp.status().is_success()
        {
            let matches_token = resp
                .headers()
                .get(COMPAT_TOKEN_HEADER)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v == token);
            if matches_token {
                return Ok(());
            }
            tracing::warn!(
                base_url,
                "healthz answered without our token (foreign server on port N)"
            );
        }
        if Instant::now() >= deadline {
            return Err(CompatCheckFailure {
                step: "spawn",
                detail: "previous binary did not answer /healthz within 10s".into(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
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
    token: &str,
    child: &mut Child,
) -> Result<(), CompatCheckFailure> {
    let client = reqwest::Client::new();
    wait_ready(&client, base_url, token, child).await?;

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
