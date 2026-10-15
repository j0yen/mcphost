//! PRD-mcphost-task-handoff-live-script AC2 (P0) — Given the server binary
//! spawned on an ephemeral local port, When `scripts/task-handoff-live.sh`
//! runs against `http://127.0.0.1:<port>`, Then stdout is exactly
//! `status: done` followed by `latency_ms: <integer>` with the integer below
//! 3000 and the exit code is 0.

use crate::common;
use crate::serve;
use serve::{ServeChild, ServeEnv};

use std::process::Command;

#[tokio::test]
async fn the_script_prints_status_done_and_a_latency_under_3000_ms() {
    let data = common::TempDataDir::new();
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data.0.clone(),
        admin_key: Some("thlive2-admin".into()),
        secret_key: Some("thlive2-secret".into()),
        extra_env: vec![("MCPHOST_LOG_LEVEL".into(), "warn".into())],
        ..ServeEnv::default()
    });
    server.wait_ready().await;

    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/task-handoff-live.sh");
    let out = Command::new("bash")
        .arg(script)
        .arg(server.base_url())
        .output()
        .expect("run script");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "exactly two lines: {stdout:?}");
    assert_eq!(lines[0], "status: done", "{stdout:?}");
    let ms: u64 = lines[1]
        .strip_prefix("latency_ms: ")
        .unwrap_or_else(|| panic!("second line: {stdout:?}"))
        .parse()
        .unwrap_or_else(|_| panic!("integer latency: {stdout:?}"));
    assert!(ms < 3000, "latency {ms} ms");
}
