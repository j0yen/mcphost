//! PRD-mcphost-golden-signals-self-probe AC1 (P0): replaying the recorded
//! bad night through the compiled `heal.toml` prints exactly one
//! `sqlite_write_ms` breach (since 13:21) and one clear, and nothing for the
//! other four signals.

use std::process::Command;

#[test]
fn replay_prints_one_sqlite_breach_since_1321_and_one_clear() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gsig_bad_night.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["signals", "replay", fixture])
        .output()
        .expect("run mcphost signals replay");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let lines: Vec<&str> = stdout.lines().collect();

    let breaches: Vec<&&str> = lines.iter().filter(|l| l.starts_with("breach ")).collect();
    let clears: Vec<&&str> = lines.iter().filter(|l| l.starts_with("clear ")).collect();
    assert_eq!(breaches.len(), 1, "stdout:\n{stdout}");
    assert_eq!(clears.len(), 1, "stdout:\n{stdout}");
    assert!(breaches[0].contains("signal=sqlite_write_ms"), "{}", breaches[0]);
    assert!(breaches[0].contains("since=2026-10-07T13:21:00Z"), "{}", breaches[0]);
    assert!(clears[0].contains("signal=sqlite_write_ms"), "{}", clears[0]);
    assert_eq!(lines.len(), 2, "no breach for the other four signals; stdout:\n{stdout}");
}
