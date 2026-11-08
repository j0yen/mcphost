//! PRD-mcphost-sandbox-timeout-clamp-live-probe AC1 -- `bash
//! scripts/live/timeout-clamp.sh --selftest` run from the repo root with no
//! network exits 0 and prints `selftest: ok`; a deleted script or a broken
//! reply parser fails here before any live judge runs.

#[test]
fn stclp_ac01_selftest_passes() {
    let out = std::process::Command::new("bash")
        .arg("scripts/live/timeout-clamp.sh")
        .arg("--selftest")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run scripts/live/timeout-clamp.sh --selftest");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.lines().any(|l| l == "selftest: ok"), "stdout: {stdout}");
}
