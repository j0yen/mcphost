//! PRD-mcphost-graceful-drain-contract-b AC3 -- Given the binary, When
//! `mcphost serve --print-drain-bound` runs with `MCPHOST_DRAIN_S` unset and
//! set to 99, Then it prints `20` and `55` respectively and exits 0.

use std::process::Command;

fn print_bound(drain_s: Option<&str>) -> (Option<i32>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.args(["serve", "--print-drain-bound"]).env_remove("MCPHOST_DRAIN_S");
    if let Some(v) = drain_s {
        cmd.env("MCPHOST_DRAIN_S", v);
    }
    let out = cmd.output().expect("run mcphost");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn gdrainb_ac03() {
    assert_eq!(print_bound(None), (Some(0), "20".to_string()));
    assert_eq!(print_bound(Some("99")), (Some(0), "55".to_string()));
}
