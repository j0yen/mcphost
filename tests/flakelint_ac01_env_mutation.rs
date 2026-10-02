//! PRD-mcphost-test-suite-flake-lints
//! AC1 (P0) — Given a fixture test file with a bare, unguarded
//! `set_var("MCPHOST_X", "1")` call, When `flake-lint.sh` runs, Then it
//! exits 1 with `env-mutation` at the right line; given the same call
//! inside `EnvGuard::set`, Then exit 0.
//!
//! Runs the real `scripts/flake-lint.sh` against a throwaway scratch
//! directory (never the real repo tree) so this exercises the actual
//! shell/grep/awk lint, not a reimplementation of its logic -- same
//! convention `tests/suite_ac2_check_catches_unregistered_file.rs` uses for
//! `gen-test-suites.sh`.
//!
//! The "bare" fixture's own call is assembled from parts (`module` below)
//! rather than spelled contiguously in this file's own source text --
//! otherwise this file's own source would itself be an `env-mutation`
//! finding under AC7's real-repo lint pass (same reasoning
//! `tests/gatedebt_4f1112d_ac4_no_test_reads_gate_receipts.rs` gives for
//! assembling its own receipt-path needle from parts).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A fresh `<scratch>/scripts/flake-lint.sh` + `<scratch>/tests/<name>` with
/// `body` as its only content, ready for `run_lint`.
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
fn bare_set_var_blocks_with_env_mutation_at_the_right_line() {
    let module = "env";
    let body = format!(
        "fn helper() {{}}\n\
         \n\
         #[test]\n\
         fn mutates_env_directly() {{\n\
         \x20   unsafe {{\n\
         \x20       std::{module}::set_var(\"MCPHOST_X\", \"1\");\n\
         \x20   }}\n\
         }}\n"
    );
    let scratch = scratch_with_fixture("ac01-bare", "bare.rs", &body);

    let out = run_lint(&scratch);
    assert!(
        !out.status.success(),
        "a bare std::env::set_var must block the lint, but it exited 0.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("tests/bare.rs:6 env-mutation"),
        "finding must name the exact file:line of the set_var call; stdout was:\n{stdout}"
    );

    fs::remove_dir_all(&scratch).ok();
}

#[test]
fn the_same_call_through_env_guard_passes() {
    let body = "#[tokio::test]\n\
                 async fn mutates_env_through_guard() {\n\
                 \x20   let _g = common::EnvGuard::set(\"MCPHOST_X\", \"1\").await;\n\
                 }\n";
    let scratch = scratch_with_fixture("ac01-guarded", "guarded.rs", body);

    let out = run_lint(&scratch);
    assert!(
        out.status.success(),
        "a call through common::EnvGuard::set must not trip the lint, but it exited \
         non-zero.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    fs::remove_dir_all(&scratch).ok();
}
