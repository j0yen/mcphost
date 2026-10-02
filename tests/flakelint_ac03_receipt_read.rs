//! PRD-mcphost-test-suite-flake-lints
//! AC3 (P0) — Given a fixture opening a path under the gate's receipt tree
//! (`target`, then `autobuilder`, then `receipts`, then a file), When the
//! lint runs, Then `receipt-read` is reported with the kindroute rule
//! cited in the message.
//!
//! Same scratch-directory convention as the other `flakelint_ac0N_*.rs`
//! files. The fixture's own literal path is assembled from parts at write
//! time (not spelled contiguously in this test file's source) for the same
//! reason `tests/gatedebt_4f1112d_ac4_no_test_reads_gate_receipts.rs`
//! assembles its own needle from parts — otherwise this file's own source
//! text would itself be a `receipt-read` finding under AC7's real-repo
//! lint pass.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch_with_fixture(tag: &str, name: &str, body: &str) -> PathBuf {
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-flakelint-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let scripts_dir = scratch.join("scripts");
    fs::create_dir_all(scratch.join("tests")).expect("mkdir tests/");
    fs::create_dir_all(&scripts_dir).expect("mkdir scripts/");
    fs::copy(
        root.join("scripts/flake-lint.sh"),
        scripts_dir.join("flake-lint.sh"),
    )
    .expect("copy flake-lint.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = scripts_dir.join("flake-lint.sh");
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&p, perm).unwrap();
    }
    fs::write(scratch.join("tests").join(name), body).expect("write fixture");
    scratch
}

/// Retries on `ExecutableFileBusy`: execing a just-chmod'd scratch script
/// can transiently race the write it followed (observed on the runner
/// box's overlay filesystem), not a real failure -- same shape as any
/// other "the OS hasn't caught up yet" flake, not a lint bug.
fn run_lint(scratch: &Path) -> std::process::Output {
    let script = scratch.join("scripts/flake-lint.sh");
    for attempt in 0..5 {
        match Command::new(&script).arg("tests").current_dir(scratch).output() {
            Ok(out) => return out,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 4 => {
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
            }
            Err(e) => panic!("run flake-lint.sh: {e}"),
        }
    }
    unreachable!()
}

#[test]
fn opening_the_gate_receipt_path_blocks_with_receipt_read_citing_kindroute() {
    let needle = ["target", "autobuilder", "receipts"].join("/");
    let body = format!(
        "#[test]\n\
         fn reads_the_gates_own_receipt() {{\n\
         \x20   let data = std::fs::read_to_string(\"{needle}/x.json\").unwrap();\n\
         \x20   assert!(!data.is_empty());\n\
         }}\n"
    );
    let scratch = scratch_with_fixture("ac03", "reads_receipt.rs", &body);

    let out = run_lint(&scratch);
    assert!(
        !out.status.success(),
        "opening a path under the gate receipt tree must block the lint, but it \
         exited 0.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("tests/reads_receipt.rs:3 receipt-read"),
        "finding must name the exact file:line of the open call; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("kindroute"),
        "the finding's message must cite the kindroute_ac08 rule; stdout was:\n{stdout}"
    );

    fs::remove_dir_all(&scratch).ok();
}
