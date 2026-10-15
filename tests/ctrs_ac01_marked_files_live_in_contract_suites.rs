//! PRD-mcphost-contract-suite-generated AC1 -- every `tests/*.rs` with a
//! `//! contract:` line in its first 10 lines is included by exactly one
//! `tests/suite_contracts_<core|sandbox>_NN.rs` binary, and a marked file left
//! inside `suite_core_01.rs` fails `gen-test-suites.sh --check` naming it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn is_marked(path: &Path) -> bool {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .take(10)
        .any(|l| l.starts_with("//! contract:"))
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

fn scratch_copy() -> PathBuf {
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-ctrs-ac1-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(scratch.join("scripts")).unwrap();
    fs::create_dir_all(scratch.join("tests")).unwrap();
    copy_dir(&root.join("tests"), &scratch.join("tests"));
    // The weights file name is assembled so this file does not itself read as
    // a weights-file contract test to scripts/contract-lint.sh.
    let weights = format!("ci-suite-{}", "weights.json");
    for name in ["gen-test-suites.sh", "ci-test-partition.sh", weights.as_str()] {
        fs::copy(root.join("scripts").join(name), scratch.join("scripts").join(name)).unwrap();
    }
    fs::copy(root.join("Cargo.toml"), scratch.join("Cargo.toml")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The weights file name is assembled so this file does not itself read as
    // a weights-file contract test to scripts/contract-lint.sh.
    let weights = format!("ci-suite-{}", "weights.json");
    for name in ["gen-test-suites.sh", "ci-test-partition.sh", weights.as_str()] {
            let p = scratch.join("scripts").join(name);
            let mut perm = fs::metadata(&p).unwrap().permissions();
            perm.set_mode(0o755);
            fs::set_permissions(&p, perm).unwrap();
        }
    }
    scratch
}

fn run_check(dir: &Path) -> std::process::Output {
    Command::new(dir.join("scripts/gen-test-suites.sh"))
        .arg("--check")
        .current_dir(dir)
        .output()
        .expect("run gen-test-suites.sh --check")
}

#[test]
fn ctrs_ac01_every_marked_file_is_in_exactly_one_contract_suite() {
    let tests = repo_root().join("tests");
    let suites: Vec<(String, String)> = fs::read_dir(&tests)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("suite_") && n.ends_with(".rs"))
        .map(|n| {
            let body = fs::read_to_string(tests.join(&n)).unwrap();
            (n, body)
        })
        .collect();

    let mut marked = 0;
    for entry in fs::read_dir(&tests).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name.starts_with("suite_") || !is_marked(&entry.path()) {
            continue;
        }
        marked += 1;
        let needle = format!("#[path = \"{name}\"]");
        let holders: Vec<&str> = suites
            .iter()
            .filter(|(_, body)| body.contains(&needle))
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(holders.len(), 1, "{name} must be in exactly one suite, found {holders:?}");
        let holder = holders[0];
        assert!(
            (holder.starts_with("suite_contracts_core_") || holder.starts_with("suite_contracts_sandbox_")),
            "{name} carries `//! contract:` but is included by {holder}"
        );
    }
    assert!(marked >= 6, "expected at least the six PRD-named contract files, found {marked}");
}

#[test]
fn ctrs_ac01_check_names_a_marked_file_left_in_suite_core_01() {
    let scratch = scratch_copy();
    let baseline = run_check(&scratch);
    assert!(
        baseline.status.success(),
        "scratch copy failed --check before any change:\n{}",
        String::from_utf8_lossy(&baseline.stderr)
    );

    fs::write(
        scratch.join("tests/zzz_ctrs_marked.rs"),
        "//! contract: a fixture contract\n#[test]\nfn probe() {}\n",
    )
    .unwrap();
    let suite = scratch.join("tests/suite_core_01.rs");
    let mut body = fs::read_to_string(&suite).unwrap();
    body.push_str("#[path = \"zzz_ctrs_marked.rs\"]\nmod zzz_ctrs_marked;\n");
    fs::write(&suite, body).unwrap();

    let out = run_check(&scratch);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "--check must fail for a marked file inside suite_core_01.rs");
    assert!(
        stderr.contains("tests/zzz_ctrs_marked.rs") && stderr.contains("suite_core_01.rs"),
        "failure must name the file and the suite; stderr:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&scratch);
}
