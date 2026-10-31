//! PRD-mcphost-docs-put-many-live-reproof
//! AC2 -- Given a loopback port the test opened and closed so nothing listens,
//! When the driver runs under `python3 -I` against `http://127.0.0.1:<port>/mcp`,
//! Then its last stdout line is `dimp_ac08 stored=0 hits=0 not-offboarded rc=2`
//! and it exits 2.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn dimpr_ac02_driver_fails_closed_against_a_dead_port() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.local_addr().expect("local addr").port()
    };
    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/live/dimp_ac08_put_many.py");
    let out = Command::new("python3")
        .args(["-I", "-B"])
        .arg(&driver)
        .arg(format!("http://127.0.0.1:{port}/mcp"))
        .output()
        .expect("python3 runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().last(),
        Some("dimp_ac08 stored=0 hits=0 not-offboarded rc=2"),
        "stdout: {stdout}; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(2));
}
