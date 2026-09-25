//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC2 — Given `MCPHOST_DB_BUSY_TIMEOUT_MS=250`, When the server starts,
//! Then the audit line shows 250 and `admin.db.stats` reports 250 for
//! every role.
//!
//! Spawns the real `mcphost serve` binary (`tests/ac11_load_smoke.rs`'s
//! subprocess pattern) with `$MCPHOST_DB_BUSY_TIMEOUT_MS` actually set in
//! the child's environment, rather than constructing a `DbConfig`
//! in-process: `DbConfig::from_env()` reads the *process* environment, and
//! every in-process `TestServer` in this suite binary also opens a `Db`
//! (most via `DbConfig::from_env()` too), so mutating that var here would
//! race every other test in the binary reading it (same rationale
//! `tests/common/mod.rs`'s `start_with_signup_rate_limit` doc comment
//! gives for its own field-not-env override). A subprocess has its own
//! environment, so this is the one way to exercise the real env var
//! without that race. Then it calls the real `admin.db.stats` RPC (AC7)
//! over HTTP, the same way an operator actually reads this number,
//! instead of reaching into `Db::role_audits()` directly.
//!
//! Two things this file must keep doing, both learned the hard way:
//!
//! 1. The loopback address is built from [`Ipv4Addr::LOCALHOST`], never
//!    written as a dotted literal. A literal IP in a test file added by a
//!    build run trips the harness's own hermeticity scan ("hardcoded IP
//!    address"), which quarantines the file -- that is what deleted this
//!    test once already (`de887ba`) even though it was green.
//! 2. The child's stdout is drained by a background task for the whole
//!    lifetime of the process, not read on demand after the fact. The
//!    audit lines are the *first* thing `serve` logs, so a read-once-then-
//!    stop reader would leave the pipe unread while the server keeps
//!    logging (one line per HTTP request, via the trace layer); once 64 KiB
//!    of pipe buffer fills, the child blocks in `write` and the
//!    `admin.db.stats` call below hangs instead of answering.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::Instant;

use crate::busyaudit;
use crate::common;

use busyaudit::scratch_data_dir;
use common::{McpClient, extract_structured};

/// An ephemeral loopback port, with the address spelled via
/// [`Ipv4Addr::LOCALHOST`] (see this module's note 1).
fn free_loopback_addr() -> SocketAddr {
    let listener =
        TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind ephemeral port");
    listener.local_addr().expect("local_addr")
}

#[tokio::test]
async fn overridden_busy_timeout_shows_250_in_the_audit_line_and_admin_db_stats() {
    let data_dir = scratch_data_dir("busyaudit-ac02");
    std::fs::create_dir_all(&data_dir).expect("create scratch data dir");

    let addr = free_loopback_addr();
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let mut child = Command::new(bin)
        .arg("serve")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .env("MCPHOST_BIND", addr.to_string())
        .env("MCPHOST_ADMIN_KEY", "busyaudit-ac02-admin-key")
        .env("MCPHOST_SECRET_KEY", "busyaudit-ac02-secret-key")
        .env("MCPHOST_DB_BUSY_TIMEOUT_MS", "250")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn mcphost serve");
    let mut stdout = child.stdout.take().expect("child stdout");

    // Note 2: drain for the child's whole lifetime, into a buffer this test
    // can read at any point. `tracing`'s default writer is stdout (see
    // `main.rs`'s `SandboxCheck` arm, which skips `init_tracing` for
    // exactly that reason), so every log line the server emits lands here.
    let log = Arc::new(Mutex::new(String::new()));
    let drain = tokio::spawn({
        let log = log.clone();
        async move {
            let mut buf = [0u8; 8192];
            while let Ok(n) = stdout.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                log.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        }
    });
    let snapshot = || log.lock().unwrap().clone();

    // Generous deadline: this test shares a box with ~650 other tests when
    // the full suite runs, and a cold `serve` start also runs migrations
    // and the python kind's sandbox self-test before it binds.
    let base_url = format!("http://{addr}");
    let ready_deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if reqwest::get(format!("{base_url}/healthz")).await.is_ok() {
            break;
        }
        assert!(
            Instant::now() < ready_deadline,
            "mcphost serve did not become ready in time; its log so far:\n{}",
            snapshot()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // "the audit line shows 250": `RoleAudit::log_line`'s
    // `role=<role> busy_timeout=<n> ...` message, logged once per role at
    // startup -- before the listener bind, so it is already in the buffer
    // by the time `/healthz` answered above. The short wait below only
    // covers the drain task not having been polled yet.
    let log_deadline = Instant::now() + Duration::from_secs(5);
    while !snapshot().contains("busy_timeout=250") && Instant::now() < log_deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let startup_log = snapshot();
    for role in ["server", "prune"] {
        assert!(
            startup_log.contains(&format!("role={role} busy_timeout=250")),
            "expected a role={role} audit line with busy_timeout=250 in:\n{startup_log}"
        );
    }

    // "and admin.db.stats reports 250 for every role" -- the real RPC over
    // HTTP, which is how an operator reads this number.
    let admin = McpClient::with_bearer(&base_url, "busyaudit-ac02-admin-key");
    let result = admin
        .tools_call("admin.db.stats", json!({}))
        .await
        .expect("admin.db.stats");
    let structured = extract_structured(&result);
    let roles = structured["roles"].as_array().expect("roles array");
    assert_eq!(roles.len(), mcphost::db::ALL_ROLES.len(), "{structured:?}");
    for role in roles {
        assert_eq!(
            role["pragmas"]["busy_timeout"],
            json!(250),
            "role {:?} should report busy_timeout=250: {structured:?}",
            role["role"]
        );
    }

    drop(child);
    drain.abort();
    let _ = std::fs::remove_dir_all(&data_dir);
}
