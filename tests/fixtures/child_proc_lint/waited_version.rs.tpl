use std::process::Command;
#[test]
fn version_runs_to_completion() {
    let out = Command::new(bin).arg("--version").output().unwrap();
    assert!(out.status.success());
}
