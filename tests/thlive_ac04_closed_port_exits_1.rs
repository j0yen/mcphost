//! PRD-mcphost-task-handoff-live-script AC4 (P0) — Given a base URL whose
//! port is closed, When the script runs, Then it exits 1 within 20 s and
//! stdout is one line starting `status: error reason:`.

use std::net::TcpListener;
use std::process::Command;

#[test]
fn a_closed_port_exits_1_with_one_error_line() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/task-handoff-live.sh");
    let out = crate::deadline_budget!(20000, {
        Command::new("bash")
            .arg(script)
            .arg(format!("http://127.0.0.1:{port}"))
            .output()
            .expect("run script")
    });
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(stdout.lines().count(), 1, "one line: {stdout:?}");
    assert!(stdout.starts_with("status: error reason:"), "{stdout:?}");
}
