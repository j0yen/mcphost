//! AC9 — Given `deploy/mcphost-emit-meter.timer`, When `systemd-analyze
//! verify` runs on both unit files, Then it exits clean, and the README
//! operator section documents installation.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// `systemd-analyze verify` does not merely parse the unit file -- it stats
/// the `%h`-expanded `ExecStart=` target and fails verification if that
/// path is not an executable file on disk, `-`-prefix or not. That is a
/// property of the running host's home directory, not of the unit file
/// under test, so a fresh checkout (or a build box that has never `cargo
/// install`ed `mcphost` to `~/.local/bin`) fails this test on a unit file
/// that is actually correct. Stand a placeholder executable up at that
/// exact path for the duration of the test, but only when nothing is
/// there already -- if the real binary is already installed (a
/// developer's own machine, post-install), we verify against it and touch
/// nothing.
struct ExecStartStub {
    path: PathBuf,
    file_created: bool,
    bin_dir_created: bool,
    local_dir_created: bool,
}

impl ExecStartStub {
    fn ensure() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        let local_dir = Path::new(&home).join(".local");
        let bin_dir = local_dir.join("bin");
        let path = bin_dir.join("mcphost");
        if path.exists() {
            return Some(Self {
                path,
                file_created: false,
                bin_dir_created: false,
                local_dir_created: false,
            });
        }
        let local_dir_created = !local_dir.exists();
        let bin_dir_created = !bin_dir.exists();
        std::fs::create_dir_all(&bin_dir).ok()?;
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").ok()?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()?;
        Some(Self {
            path,
            file_created: true,
            bin_dir_created,
            local_dir_created,
        })
    }
}

impl Drop for ExecStartStub {
    fn drop(&mut self) {
        if self.file_created {
            let _ = std::fs::remove_file(&self.path);
        }
        if self.bin_dir_created {
            if let Some(bin_dir) = self.path.parent() {
                let _ = std::fs::remove_dir(bin_dir);
            }
        }
        if self.local_dir_created {
            if let Some(local_dir) = self.path.parent().and_then(Path::parent) {
                let _ = std::fs::remove_dir(local_dir);
            }
        }
    }
}

/// `systemd-analyze verify` isn't installed on every CI image; skip rather
/// than fail when it's genuinely absent so this test only ever fails on a
/// real unit-file defect, not environment shape. `--user` matches this
/// crate's `mcphost-backup.service` sibling convention (user-scope units
/// under `~/.config/systemd/user/`, `%h`-relative paths). The `ExecStart=`
/// binary itself is stubbed in for the duration (see `ExecStartStub`) so
/// this only ever fails on a real unit-file defect, not on "mcphost isn't
/// installed on this box yet."
#[test]
fn deploy_units_pass_systemd_analyze_verify() {
    if Command::new("systemd-analyze").arg("--version").output().is_err() {
        eprintln!("systemd-analyze not on PATH; skipping (see doc comment)");
        return;
    }
    let _exec_start_stub = ExecStartStub::ensure();
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
