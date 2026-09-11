//! AC9 — Given `deploy/mcphost-emit-meter.timer`, When `systemd-analyze
//! verify` runs against a hermetic copy of the repo's own units, Then it
//! exits clean on any host, and the README operator section documents
//! installation.
//!
//! PRD-mcphost-tests-host-independence requirement 3: the prior version of
//! this test ran `systemd-analyze verify --user` directly against
//! `deploy/*.{service,timer}` in place, so its verdict depended on
//! whatever unit tree the host it ran on happened to have (it failed on
//! both sides of the first RedBaron/box parity for exactly this reason).
//! Copying the two units into a throwaway directory and pointing
//! `SYSTEMD_UNIT_PATH` at it (with a trailing `:` so systemd still falls
//! back to its normal search path for stock targets these units reference,
//! e.g. `network-online.target`, per the PRD's technical-considerations
//! note) makes the verdict depend only on the files this repo ships, not on
//! the host's own unit tree. `systemd-analyze` is a required tool in the
//! gate's own tool list, so its absence is a hard failure now, never a
//! silent skip.
//!
//! One more host fact hid inside "verify" beyond the unit search path:
//! `ExecStart=%h/.local/bin/mcphost` expands `%h` to the *invoking* user's
//! home directory, and `verify` stats that path for an executable file --
//! true on a box that has already installed mcphost there (RedBaron), a
//! hard failure on one that hasn't (the box's build user, and any host
//! "with no units installed" per this AC). A `HOME` override pointing at a
//! throwaway directory containing a dummy executable stub at
//! `.local/bin/mcphost` makes that check pass on its own file's presence,
//! never on whether a real deploy happened on this machine -- exactly the
//! "verifies the repo's units from a temp directory" AC3 asks for.

use std::path::{Path, PathBuf};
use std::process::Command;

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const UNITS: [&str; 2] = ["mcphost-emit-meter.service", "mcphost-emit-meter.timer"];

/// A directory under the OS temp dir, unique per call, removed on drop --
/// same shape as `tests/common/mod.rs`'s `TempDataDir`, kept local here so
/// this file stays self-contained (it does not otherwise need `mod common`).
struct TempUnitDir(PathBuf);

impl TempUnitDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mcphost-unit-verify-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp unit dir");
        Self(dir)
    }
}

impl Drop for TempUnitDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn deploy_units_pass_systemd_analyze_verify() {
    if Command::new("systemd-analyze").arg("--version").output().is_err() {
        panic!(
            "systemd-analyze is not on PATH -- it is a required tool in this gate's own tool \
             list, so its absence is a real environment defect, not something to skip past"
        );
    }

    let tmp = TempUnitDir::new();
    let mut copied = Vec::new();
    for unit in UNITS {
        let src = crate_root().join("deploy").join(unit);
        assert!(src.exists(), "{unit} must exist under deploy/");
        let dst = tmp.0.join(unit);
        std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copy {unit} into temp dir: {e}"));
        copied.push(dst);
    }

    // Dummy `HOME` so `ExecStart=%h/.local/bin/mcphost` resolves to a stub
    // this test controls, not to whatever this host's real user happens (or
    // doesn't happen) to have installed -- see the module doc comment.
    let fake_home = tmp.0.join("home");
    let stub_bin = fake_home.join(".local").join("bin");
    std::fs::create_dir_all(&stub_bin).expect("create fake HOME/.local/bin");
    let stub_mcphost = stub_bin.join("mcphost");
    std::fs::write(&stub_mcphost, "#!/bin/sh\nexit 0\n").expect("write mcphost stub");
    let mut perms = std::fs::metadata(&stub_mcphost)
        .expect("stat mcphost stub")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&stub_mcphost, perms).expect("chmod mcphost stub");

    let output = Command::new("systemd-analyze")
        .args(["verify", "--man=no", "--generators=no"])
        .args(&copied)
        .env("SYSTEMD_UNIT_PATH", format!("{}:", tmp.0.display()))
        .env("HOME", &fake_home)
        .output()
        .expect("run systemd-analyze verify");
    assert!(
        output.status.success(),
        "units failed systemd-analyze verify from hermetic dir {}:\nstdout: {}\nstderr: {}",
        tmp.0.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn readme_documents_emit_meter_installation() {
    let readme = std::fs::read_to_string(crate_root().join("README.md")).expect("read README.md");
    assert!(
        readme.contains("mcphost-emit-meter"),
        "README.md must document installing the emit-meter timer"
    );
}
