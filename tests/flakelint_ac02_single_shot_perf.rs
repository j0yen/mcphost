//! PRD-mcphost-test-suite-flake-lints
//! AC2 (P0) — Given a fixture asserting `start.elapsed() <
//! Duration::from_millis(50)` once, When the lint runs, Then
//! `single-shot-perf` is reported; given the same budget inside
//! `perf_budget!(50, …)`, Then exit 0.
//!
//! Same scratch-directory convention as
//! `tests/flakelint_ac01_env_mutation.rs`: runs the real
//! `scripts/flake-lint.sh` against throwaway fixture text, never the real
//! repo tree.
//!
//! The "bare" fixture's own single-measurement perf line is split across a
//! source-level line break that the fixture's *string value* doesn't carry
//! (Rust folds a backslash-newline-whitespace sequence inside a string
//! literal to nothing) -- otherwise this file's own source would itself
//! trip the rule AC7's real-repo lint pass checks for.

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
fn single_shot_elapsed_assert_blocks_with_single_shot_perf() {
    let body = "#[test]\n\
                 fn measures_once() {\n\
                 \x20   let start = std::time::Instant::now();\n\
                 \x20   do_work();\n\
                 \x20   assert!(start.elapsed() \
                 < std::time::Duration::from_millis(50));\n\
                 }\n";
    let scratch = scratch_with_fixture("ac02-bare", "bare_perf.rs", body);

    let out = run_lint(&scratch);
    assert!(
        !out.status.success(),
        "a single elapsed()-vs-Duration assert! must block the lint, but it exited \
         0.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("tests/bare_perf.rs:5 single-shot-perf"),
        "finding must name the exact file:line of the assert!; stdout was:\n{stdout}"
    );

    fs::remove_dir_all(&scratch).ok();
}

#[test]
fn the_same_budget_through_perf_budget_macro_passes() {
    let body = "#[test]\n\
                 fn measures_with_budget() {\n\
                 \x20   crate::perf_budget!(50, {\n\
                 \x20       do_work();\n\
                 \x20   });\n\
                 }\n";
    let scratch = scratch_with_fixture("ac02-guarded", "guarded_perf.rs", body);

    let out = run_lint(&scratch);
    assert!(
        out.status.success(),
        "a perf_budget!(...) call site must not trip the lint, but it exited \
         non-zero.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    fs::remove_dir_all(&scratch).ok();
}
