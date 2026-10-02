//! PRD-mcphost-test-suite-flake-lints
//! AC4 (P0) — Given an allow-list entry
//! `tests/legacy.rs:single-shot-perf # measured on quiet box only`, When
//! the lint runs on that file, Then exit 0 and the report lists
//! `allowed: 1`.
//!
//! Same scratch-directory convention as the other `flakelint_ac0N_*.rs`
//! files, but this one also writes a `tests/flake-lint-allow.txt` into the
//! scratch tree (ALLOW_FILE is resolved relative to the scanned TARGET, not
//! the real repo — see flake-lint.sh's own comment on that).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch_with_fixture(tag: &str, name: &str, body: &str, allow_list: &str) -> PathBuf {
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
    fs::write(scratch.join("tests/flake-lint-allow.txt"), allow_list).expect("write allow-list");
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
fn allow_listed_finding_passes_and_is_counted() {
    let body = "#[test]\n\
                 fn measures_once() {\n\
                 \x20   let start = std::time::Instant::now();\n\
                 \x20   do_work();\n\
                 \x20   assert!(start.elapsed() \
                 < std::time::Duration::from_millis(50));\n\
                 }\n";
    let scratch = scratch_with_fixture(
        "ac04",
        "legacy.rs",
        body,
        "tests/legacy.rs:single-shot-perf # measured on quiet box only\n",
    );

    let out = run_lint(&scratch);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "an allow-listed finding must exit 0, but it exited non-zero.\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        stdout.contains("allowed: 1"),
        "the report must list exactly one allowed finding; stdout was:\n{stdout}"
    );

    fs::remove_dir_all(&scratch).ok();
}
