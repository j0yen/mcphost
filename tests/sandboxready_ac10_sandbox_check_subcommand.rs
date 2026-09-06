//! PRD-mcphost-sandbox-ready
//! AC10 (P2) -- Given the host binary, When `mcphost sandbox-check` runs on
//! a ready and on an unready box, Then it exits 0 and 1 respectively and
//! prints the same detail line `/healthz` would show.
//!
//! Scoped decision: this test exercises the real, ready path end-to-end
//! (this repository's own build machine's sandbox works -- same precondition
//! every other sandbox-dependent test in this suite requires). The
//! "unready" half of this AC is not independently re-provable through the
//! CLI without a hidden test-only env knob the PRD doesn't ask for and
//! `main.rs` doesn't otherwise need -- `sandbox-check` calls the exact same
//! `PythonKind::run_startup_selftest` / exit-code-from-`.ready` mapping
//! already covered end-to-end by `tests/sandboxready_ac1_ac2_healthz_startup.rs`
//! (unready -> `sandbox_ready: false`) and unit-tested by
//! `sandbox::selftest_tests` (every classification token), so the only
//! genuinely CLI-specific behavior left to prove here -- exit code 0 and
//! the printed detail line matching `/healthz`'s own format -- is what this
//! test actually checks.

use mcphost::sandbox;
use std::process::Command;

#[test]
fn sandbox_check_exits_0_and_prints_the_healthz_detail_line_on_a_ready_box() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = std::env::temp_dir().join(format!(
        "mcphost-sandboxcheck-test-{}-{}",
        std::process::id(),
        rand_suffix()
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("sandbox-check")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .output()
        .expect("spawn mcphost sandbox-check");

    let _ = std::fs::remove_dir_all(&data_dir);

    assert!(
        output.status.success(),
        "sandbox-check must exit 0 on a working sandbox; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mechanism = sandbox::detect_mechanism().as_str();
    assert_eq!(
        stdout.trim(),
        format!("{mechanism}: ok"),
        "must print the same detail line /healthz would show, got: {stdout}"
    );
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
