//! contract: reads scripts/live/osreach_import_reach.py
//! PRD-mcphost-import-dry-run-live-real-export
//! AC2 (P0) -- Given `scripts/live/osreach_import_reach.py`, When it runs with
//! `--plan` under `HTTPS_PROXY=http://127.0.0.1:9`, Then it exits 0, prints the
//! sequence source signup, state set, export, wait, destination signup, Claude
//! Code `host_import` dry run, `host_doctor`, key grep on the stream, offboard
//! both, prints the evidence line template with `export_run_id=`, and makes no
//! network call.

use std::process::Command;

pub const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/osreach_import_reach.py");

#[test]
fn osrx_ac02_plan_prints_the_sequence_and_template_with_no_network() {
    // A dead proxy: any attempted request would fail the run or print an error.
    let out = Command::new("python3")
        .arg(SCRIPT)
        .arg("--plan")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("MCPHOST_BASE_URL", "http://127.0.0.1:9")
        .output()
        .expect("run osreach_import_reach.py");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout} {}", String::from_utf8_lossy(&out.stderr));

    let steps = [
        "source signup",
        "state set",
        "export",
        "wait",
        "destination signup",
        "host_import dry run",
        "host_doctor",
        "key grep on the stream",
        "offboard both",
    ];
    let mut from = 0;
    for step in steps {
        let at = stdout[from..].find(step).unwrap_or_else(|| panic!("`{step}` missing or out of order in:\n{stdout}"));
        from += at + step.len();
    }
    let evidence = stdout.lines().find(|l| l.starts_with("evidence:")).expect("evidence line template");
    for field in ["tool_results=", "mismatch=", "is_error=", "export_run_id=", "unreached=", "cap=", "offboarded="] {
        assert!(evidence.contains(field), "{field} missing from {evidence}");
    }
    assert!(out.stderr.is_empty(), "plan mode writes nothing to stderr (no connection error)");
}
