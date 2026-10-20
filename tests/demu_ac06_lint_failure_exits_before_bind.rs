//! PRD-mcphost-dev-emulator
//! AC6 (P0) -- Given a directory that fails `lint`, When `mcphost dev`
//! starts, Then it exits 1 before binding a port and prints the
//! `invalid_spec` finding.

use crate::common::ToolDirFixture;
use serde_json::Value;
use tokio::process::Command;

#[tokio::test]
async fn demu_ac06_a_directory_that_fails_lint_exits_1_without_binding_a_port() {
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tool.toml", "[tool]\nname = \"report\"\nkind = \"python\"\n\n[spec]\nno_such_spec_field = 1\n");
    dir.write("tool.py", "def main(args):\n    return {}\n");

    // The directory's own lint names the finding the emulator must repeat.
    let lint = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["tool", "lint"])
        .arg(&dir.0)
        .output()
        .await
        .expect("run mcphost tool lint");
    assert_eq!(lint.status.code(), Some(1), "the fixture fails lint");

    // A port nothing is listening on, so a later connect proves nothing bound it.
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe port");
        probe.local_addr().expect("addr").port()
    };
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("dev")
        .arg(&dir.0)
        .args(["--no-sandbox", "--port", &port.to_string()])
        .output()
        .await
        .expect("run mcphost dev");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}");
    let finding: Value = serde_json::from_str(stdout.lines().last().unwrap_or_default()).expect("the finding is JSON");
    assert_eq!(finding["code"], "invalid_spec", "{finding}");
    assert_eq!(finding["ok"], false, "{finding}");
    assert!(stdout.contains("no_such_spec_field"), "the finding names the bad field: {stdout}");
    assert!(!stdout.contains("serving http"), "no listening line before the finding: {stdout}");
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "nothing is listening on port {port}"
    );
}
