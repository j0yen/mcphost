//! PRD-mcphost-child-process-test-harness AC4 — the leg-17 path: a child that
//! never answers `/healthz` makes `ServeChild::wait_ready()` panic within
//! `serve_ready_timeout() + 15 s` with `stderr:` in the message, and the child
//! is gone afterwards (`kill -0 <pid>` fails). Before the helper, the
//! readiness-timeout path blocked on a live child's stderr and held the
//! 90-minute gate wall.

use crate::common::{self, EnvGuard};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::serve;
use serve::ServeChild;

fn unused_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn stalled_child_panics_with_stderr_within_deadline_and_is_killed() {
    // Hold the env lock with the loaded-host switch absent, so the deadline
    // this test computes is the one `wait_ready()` uses.
    let _env = EnvGuard::clear("MCPHOST_PERF_SKIP").await;
    let budget = common::serve_ready_timeout();

    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo stalled-child-says-hi >&2; exec sleep 600"]);
    let mut server = ServeChild::spawn_command(cmd, unused_port());
    let pid = server.child_id();

    let started = Instant::now();
    let joined = tokio::spawn(async move { server.wait_ready().await }).await;
    let took = started.elapsed();

    let err = joined.expect_err("wait_ready must panic for a child that never answers");
    let msg = match err.try_into_panic() {
        Ok(p) => p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .expect("string panic payload"),
        Err(e) => panic!("task did not panic: {e}"),
    };
    assert!(msg.contains("stderr:"), "panic message lacks `stderr:`: {msg}");
    assert!(
        msg.contains("stalled-child-says-hi"),
        "panic message lacks the child's stderr: {msg}"
    );
    let limit = budget + Duration::from_secs(15);
    assert!(took < limit, "wait_ready took {took:?}, limit {limit:?}");

    let alive = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("run kill -0")
        .success();
    assert!(!alive, "child pid {pid} is still alive after the readiness failure");
}
