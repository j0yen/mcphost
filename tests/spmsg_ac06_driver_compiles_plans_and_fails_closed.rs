//! contract: reads scripts/live/spmsg_ac08.py
//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC6 (P0) — Given the crate root, When `tests/spmsg_ac06_*.rs` runs, Then
//! `scripts/live/spmsg_ac08.py` exists, `python3 -I -m py_compile` on it exits 0,
//! `--plan` prints its steps and the evidence template, and against a loopback
//! port the test opened and closed so nothing listens it exits 1 with last line
//! `refused=0 field_named=0 spec_in_message=0 republished=0 removed=0
//! offboarded=0`.

use crate::common;
use std::path::Path;
use std::process::{Command, Output};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/spmsg_ac08.py");

fn python(args: &[&str]) -> Output {
    Command::new("python3")
        .arg("-I")
        .args(args)
        // A dead proxy: a loopback request must not use it, a stray remote one fails.
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .output()
        .expect("run python3")
}

fn last_line(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or_default().to_string()
}

#[test]
fn spmsg_ac06() {
    assert!(Path::new(SCRIPT).is_file(), "{SCRIPT} exists");

    // Compile a copy in scratch space, so no `__pycache__` lands in the tree.
    let scratch = common::TempDataDir::new();
    std::fs::create_dir_all(&scratch.0).expect("scratch dir");
    let copy = scratch.0.join("spmsg_ac08.py");
    std::fs::copy(SCRIPT, &copy).expect("copy driver");
    let compiled = python(&["-m", "py_compile", copy.to_str().unwrap()]);
    assert!(compiled.status.success(), "py_compile: {}", String::from_utf8_lossy(&compiled.stderr));

    let plan = python(&[SCRIPT, "--plan"]);
    let stdout = String::from_utf8_lossy(&plan.stdout).to_string();
    assert!(plan.status.success(), "{stdout} {}", String::from_utf8_lossy(&plan.stderr));
    let mut from = 0;
    for step in ["signup", "host_tool_publish", "method", "minimal spec", "host_tool_publish", "host_tool_remove", "host_self_offboard"] {
        let at = stdout[from..].find(step).unwrap_or_else(|| panic!("`{step}` missing or out of order in:\n{stdout}"));
        from += at + step.len();
    }
    assert!(
        stdout.contains(
            "evidence: refused=<n> field_named=<n> spec_in_message=<n> republished=<n> removed=<n> offboarded=<n>"
        ),
        "{stdout}"
    );
    assert!(plan.stderr.is_empty(), "plan mode makes no connection");

    // A port that was open a moment ago and is closed now: nothing listens.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let results = scratch.0.join("results.jsonl");
    let base = format!("http://127.0.0.1:{port}");
    let out = python(&[SCRIPT, "--base-url", &base, "--results", results.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let zeros = "refused=0 field_named=0 spec_in_message=0 republished=0 removed=0 offboarded=0";
    assert_eq!(last_line(&out), zeros);
    let row: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&results).expect("results file").lines().last().expect("a row"))
            .expect("row is json");
    assert_eq!(row["summary"], zeros, "{row}");
    assert_eq!(row["result"], "fail", "{row}");
}
