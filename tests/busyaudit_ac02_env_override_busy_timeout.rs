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
//! Two things this file keeps doing, both learned the hard way:
//!
//! 1. The loopback address is never written as a dotted literal in a test
//!    file (a literal IP trips the harness's hermeticity scan, which
//!    quarantined this test once, `de887ba`); `ServeChild` owns the address.
//! 2. The child's stdout is drained for the whole
//!    lifetime (`ServeEnv::drain_stdout`, read via `stdout_log()`) of the process, not read on demand after the fact. The
//!    audit lines are the *first* thing `serve` logs, so a read-once-then-
//!    stop reader would leave the pipe unread while the server keeps
//!    logging (one line per HTTP request, via the trace layer); once 64 KiB
//!    of pipe buffer fills, the child blocks in `write` and the
//!    `admin.db.stats` call below hangs instead of answering.

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::serve;
use serve::{ServeChild, ServeEnv};

use crate::busyaudit;
use crate::common;

use busyaudit::scratch_data_dir;
use common::{McpClient, extract_structured};

#[tokio::test]
async fn overridden_busy_timeout_shows_250_in_the_audit_line_and_admin_db_stats() {
    let data_dir = scratch_data_dir("busyaudit-ac02");
    std::fs::create_dir_all(&data_dir).expect("create scratch data dir");

    // Note 2: the helper drains the child's stdout for its whole lifetime
    // (`drain_stdout`), into a buffer read at any point via `stdout_log()`.
    // `tracing`'s default writer is stdout (see `main.rs`'s `SandboxCheck`
    // arm, which skips `init_tracing` for exactly that reason), so every log
    // line the server emits lands there. `wait_ready` uses the host-aware
    // `serve_ready_timeout()`: this test shares a box with ~650 other tests
    // when the full suite runs, and a cold `serve` start also runs
    // migrations and the python kind's sandbox self-test before it binds.
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data_dir.clone(),
        admin_key: Some("busyaudit-ac02-admin-key".into()),
        secret_key: Some("busyaudit-ac02-secret-key".into()),
        extra_env: vec![("MCPHOST_DB_BUSY_TIMEOUT_MS".into(), "250".into())],
        drain_stdout: true,
    });
    server.wait_ready().await;
    let base_url = server.base_url();
    let snapshot = || server.stdout_log();

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
    // A box shared with the whole suite can answer the first call(s) with a
    // transient tool error (no `roles`); retry until the payload arrives and
    // keep the raw result for the failure message.
    let stats_deadline = Instant::now() + Duration::from_secs(20);
    let (structured, roles) = loop {
        let outcome = admin.tools_call("admin.db.stats", json!({})).await;
        let raw = format!("{outcome:?}");
        if let Ok(result) = outcome {
            let structured = extract_structured(&result);
            if let Some(roles) = structured["roles"].as_array().cloned() {
                break (structured, roles);
            }
        }
        assert!(
            Instant::now() < stats_deadline,
            "admin.db.stats never returned a roles array; last result: {raw}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert_eq!(roles.len(), mcphost::db::ALL_ROLES.len(), "{structured:?}");
    for role in roles {
        assert_eq!(
            role["pragmas"]["busy_timeout"],
            json!(250),
            "role {:?} should report busy_timeout=250: {structured:?}",
            role["role"]
        );
    }

    drop(server);
    let _ = std::fs::remove_dir_all(&data_dir);
}
