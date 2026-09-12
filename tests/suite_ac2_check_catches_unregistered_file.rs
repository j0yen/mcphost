//! PRD-mcphost-test-suite-consolidation
//! AC2 (P0) -- Given a new file `tests/zzz_ac1_probe.rs` that is not in any
//! suite, When `gen-test-suites.sh --check` runs, Then it exits non-zero
//! naming `tests/zzz_ac1_probe.rs`; after regenerating, it exits 0 and the
//! test is listed.
//!
//! Runs the real scripts against a throwaway COPY of `tests/` + the two
//! generator scripts + `Cargo.toml` (never the real repo -- this test must
//! not mutate the tree it's compiled from) in a temp directory, so it
//! exercises the actual shell/python `gen-test-suites.sh`, not a
//! reimplementation of its logic.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Copy `tests/`, `scripts/gen-test-suites.sh`, `scripts/ci-test-partition.sh`
/// and `Cargo.toml` into a fresh temp dir. `gen-test-suites.sh` only reads
/// file text (never compiles anything), so this is enough for it to run for
/// real without touching the crate this test is itself compiled from.
fn scratch_copy() -> PathBuf {
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-suite-ac2-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let scripts_dir = scratch.join("scripts");
    fs::create_dir_all(scratch.join("tests")).expect("mkdir tests/");
    fs::create_dir_all(&scripts_dir).expect("mkdir scripts/");

    copy_dir(&root.join("tests"), &scratch.join("tests"));
    fs::copy(
        root.join("scripts/gen-test-suites.sh"),
        scripts_dir.join("gen-test-suites.sh"),
    )
    .expect("copy gen-test-suites.sh");
    fs::copy(
        root.join("scripts/ci-test-partition.sh"),
        scripts_dir.join("ci-test-partition.sh"),
    )
    .expect("copy ci-test-partition.sh");
    fs::copy(root.join("Cargo.toml"), scratch.join("Cargo.toml")).expect("copy Cargo.toml");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["gen-test-suites.sh", "ci-test-partition.sh"] {
            let p = scripts_dir.join(name);
            let mut perm = fs::metadata(&p).unwrap().permissions();
            perm.set_mode(0o755);
            fs::set_permissions(&p, perm).unwrap();
        }
    }

    scratch
}

fn copy_dir(src: &Path, dst: &Path) {
    for entry in fs::read_dir(src).expect("read_dir").flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            fs::create_dir_all(&dest).unwrap();
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

fn run_check(scratch: &Path) -> std::process::Output {
    Command::new(scratch.join("scripts/gen-test-suites.sh"))
        .arg("--check")
        .current_dir(scratch)
        .output()
        .expect("run gen-test-suites.sh --check")
}

fn run_write(scratch: &Path) -> std::process::Output {
    Command::new(scratch.join("scripts/gen-test-suites.sh"))
        .current_dir(scratch)
        .output()
        .expect("run gen-test-suites.sh")
}

#[test]
fn unregistered_file_fails_check_then_passes_after_regen() {
    let scratch = scratch_copy();

    // Baseline: the untouched copy should already be check-clean (it's a
    // copy of the real, already-generated repo).
    let baseline = run_check(&scratch);
    assert!(
        baseline.status.success(),
        "scratch copy of the real repo failed --check before any change was \
         made -- the fixture itself is broken.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&baseline.stdout),
        String::from_utf8_lossy(&baseline.stderr),
    );

    // Introduce the new, unregistered file.
    let probe = scratch.join("tests/zzz_ac1_probe.rs");
    fs::write(
        &probe,
        "#[test]\nfn probe() {\n    assert!(true);\n}\n",
    )
    .expect("write zzz_ac1_probe.rs");

    let after_new_file = run_check(&scratch);
    assert!(
        !after_new_file.status.success(),
        "gen-test-suites.sh --check should have failed once an unregistered \
         file exists, but it exited 0"
    );
    let stderr = String::from_utf8_lossy(&after_new_file.stderr);
    assert!(
        stderr.contains("zzz_ac1_probe.rs"),
        "the --check failure must name tests/zzz_ac1_probe.rs; stderr was:\n{stderr}"
    );

    // Regenerate: --check must now pass, and the file must be included.
    let write_out = run_write(&scratch);
    assert!(
        write_out.status.success(),
        "gen-test-suites.sh (write mode) failed after adding the probe file.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&write_out.stdout),
        String::from_utf8_lossy(&write_out.stderr),
    );

    let after_regen = run_check(&scratch);
    assert!(
        after_regen.status.success(),
        "gen-test-suites.sh --check still failed after regenerating.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&after_regen.stdout),
        String::from_utf8_lossy(&after_regen.stderr),
    );

    let referenced = fs::read_dir(scratch.join("tests"))
        .expect("read tests/")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("suite_"))
        .any(|e| {
            fs::read_to_string(e.path())
                .map(|c| c.contains(r#"#[path = "zzz_ac1_probe.rs"]"#))
                .unwrap_or(false)
        });
    assert!(
        referenced,
        "zzz_ac1_probe.rs was not found in any suite_*.rs file after regeneration"
    );

    fs::remove_dir_all(&scratch).ok();
}
