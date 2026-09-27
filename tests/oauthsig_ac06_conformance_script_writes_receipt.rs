//! PRD-mcphost-oauth-demand-signal
//! AC6 (P1) — Given a host started locally, When
//! `scripts/oauth-conformance.sh http://127.0.0.1:<port>/mcp` runs with
//! `npx` available, Then it writes `docs/receipts/oauth-conformance-<date>.md`
//! and exits 0 on pass, non-zero on any failed scenario; When `npx` is
//! absent, Then it exits 2 with a one-line reason.
//!
//! The script is copied into a scratch directory shaped like this repo's
//! root (`<scratch>/scripts/oauth-conformance.sh`, an empty `<scratch>/docs/`)
//! so it writes its receipt there, never into this repo's own
//! `docs/receipts/`. `npx` itself is a fake shell script this test controls
//! (same "tests never reach the network" convention every other outbound
//! client in this crate follows -- `FakeBillingClient`/`FakeEmailClient`),
//! not the real `@modelcontextprotocol/conformance` package: that package
//! needs network access this sandboxed test environment doesn't have, and
//! the script's own job here is shelling out and handling the exit code /
//! writing the report, not the conformance tool's own scenario logic.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A minimal PATH with no plausible `npx` on it -- excludes every common
/// Node install location (`~/.nvm`, `/usr/local/bin`, `~/.local/bin`,
/// `/opt/homebrew/bin`, `~/.cargo/bin`) rather than trusting this build
/// box happens to have none installed.
const NPX_FREE_PATH: &str = "/usr/bin:/bin";

fn scratch_repo_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mcphost-oauthsig-ac06-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("scripts")).expect("create scripts dir");
    fs::create_dir_all(dir.join("docs/receipts")).expect("create docs/receipts dir");
    let script = include_str!("../scripts/oauth-conformance.sh");
    let dest = dir.join("scripts/oauth-conformance.sh");
    fs::write(&dest, script).expect("write script copy");
    let mut perms = fs::metadata(&dest).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest, perms).expect("chmod +x");
    dir
}

/// Writes a fake `npx` (this test's own stand-in for the real conformance
/// tool) into `<dir>/bin/npx`, exiting `exit_code` and printing one
/// recognizable line to stdout.
fn write_fake_npx(dir: &Path, exit_code: i32) {
    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let npx_path = bin_dir.join("npx");
    fs::write(
        &npx_path,
        format!(
            "#!/usr/bin/env bash\necho 'fake conformance harness: scenarios ran'\nexit {exit_code}\n"
        ),
    )
    .expect("write fake npx");
    let mut perms = fs::metadata(&npx_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&npx_path, perms).expect("chmod +x fake npx");
}

fn receipt_files(scratch: &Path) -> Vec<PathBuf> {
    fs::read_dir(scratch.join("docs/receipts"))
        .expect("read docs/receipts")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("oauth-conformance-") && n.ends_with(".md"))
        })
        .collect()
}

#[test]
fn npx_absent_exits_2_with_a_one_line_reason_and_writes_no_receipt() {
    let scratch = scratch_repo_root("npx-absent");

    let output = Command::new(scratch.join("scripts/oauth-conformance.sh"))
        .arg("http://127.0.0.1:1/mcp")
        .env("PATH", NPX_FREE_PATH)
        .output()
        .expect("run oauth-conformance.sh");

    assert_eq!(output.status.code(), Some(2), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "expected exactly one line of stderr: {stderr:?}");
    assert!(stderr.to_lowercase().contains("npx"), "reason must name npx: {stderr:?}");
    assert!(receipt_files(&scratch).is_empty(), "no receipt should be written when npx is absent");

    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn npx_present_and_passing_exits_0_and_writes_the_receipt() {
    let scratch = scratch_repo_root("npx-pass");
    write_fake_npx(&scratch, 0);
    let path = format!("{}:{NPX_FREE_PATH}", scratch.join("bin").display());

    let output = Command::new(scratch.join("scripts/oauth-conformance.sh"))
        .arg("http://127.0.0.1:1/mcp")
        .env("PATH", path)
        .output()
        .expect("run oauth-conformance.sh");

    assert_eq!(output.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let receipts = receipt_files(&scratch);
    assert_eq!(receipts.len(), 1, "expected exactly one receipt: {receipts:?}");
    let contents = fs::read_to_string(&receipts[0]).expect("read receipt");
    assert!(contents.contains("http://127.0.0.1:1/mcp"), "receipt must name the target: {contents}");
    assert!(contents.contains("fake conformance harness"), "receipt must carry the tool's own output: {contents}");

    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn npx_present_and_failing_exits_non_zero_and_still_writes_the_receipt() {
    let scratch = scratch_repo_root("npx-fail");
    write_fake_npx(&scratch, 1);
    let path = format!("{}:{NPX_FREE_PATH}", scratch.join("bin").display());

    let output = Command::new(scratch.join("scripts/oauth-conformance.sh"))
        .arg("http://127.0.0.1:1/mcp")
        .env("PATH", path)
        .output()
        .expect("run oauth-conformance.sh");

    assert_ne!(output.status.code(), Some(0), "a failed scenario must not exit 0");
    let receipts = receipt_files(&scratch);
    assert_eq!(receipts.len(), 1, "a failed run still writes an inspectable receipt: {receipts:?}");

    let _ = fs::remove_dir_all(&scratch);
}
