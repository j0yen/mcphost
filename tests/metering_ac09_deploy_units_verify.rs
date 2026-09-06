//! AC9 — Given `deploy/mcphost-emit-meter.timer`, When `systemd-analyze
//! verify` runs on both unit files, Then it exits clean, and the README
//! operator section documents installation.

use std::path::Path;
use std::process::Command;

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// `systemd-analyze verify` isn't installed on every CI image; skip rather
/// than fail when it's genuinely absent so this test only ever fails on a
/// real unit-file defect, not environment shape. `--user` matches this
/// crate's `mcphost-backup.service` sibling convention (user-scope units
/// under `~/.config/systemd/user/`, `%h`-relative paths).
#[test]
fn deploy_units_pass_systemd_analyze_verify() {
    if Command::new("systemd-analyze").arg("--version").output().is_err() {
        eprintln!("systemd-analyze not on PATH; skipping (see doc comment)");
        return;
    }
    for unit in ["mcphost-emit-meter.service", "mcphost-emit-meter.timer"] {
        let path = crate_root().join("deploy").join(unit);
        assert!(path.exists(), "{unit} must exist under deploy/");
        let output = Command::new("systemd-analyze")
            .args(["verify", "--user"])
            .arg(&path)
            .output()
            .expect("run systemd-analyze verify");
        assert!(
            output.status.success(),
            "{unit} failed systemd-analyze verify:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn readme_documents_emit_meter_installation() {
    let readme = std::fs::read_to_string(crate_root().join("README.md")).expect("read README.md");
    assert!(
        readme.contains("mcphost-emit-meter"),
        "README.md must document installing the emit-meter timer"
    );
}
