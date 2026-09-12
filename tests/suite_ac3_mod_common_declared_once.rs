//! PRD-mcphost-test-suite-consolidation
//! AC3 (P0) -- Given the consolidated layout, When `grep -c 'mod common'
//! tests/*.rs` runs over the included files, Then the count is 0 and each
//! suite file declares `mod common;` once.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn is_generated_suite(name: &str) -> bool {
    name.starts_with("suite_core_") || name.starts_with("suite_sandbox_")
}

#[test]
fn member_files_have_no_bare_mod_common_line() {
    let tests_dir = repo_root().join("tests");
    let mut offenders = Vec::new();
    for entry in fs::read_dir(&tests_dir).expect("read tests/").flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_generated_suite(&name) {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        if content
            .lines()
            .any(|l| l.trim() == "mod common;" || l.trim() == "mod ci_sandbox_support;")
        {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "these member files still declare a bare `mod common;`/`mod \
         ci_sandbox_support;` instead of `use crate::common;`/`use \
         crate::ci_sandbox_support;` (scripts/gen-test-suites.sh migrates \
         this automatically -- run it): {offenders:?}"
    );
}

#[test]
fn each_generated_suite_declares_common_at_most_once() {
    let tests_dir = repo_root().join("tests");
    let mut any_declares_common = false;
    for entry in fs::read_dir(&tests_dir).expect("read tests/").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_generated_suite(&name) || !name.ends_with(".rs") {
            continue;
        }
        let content = fs::read_to_string(entry.path()).unwrap_or_default();
        let common_count = content.lines().filter(|l| l.trim() == "mod common;").count();
        assert!(
            common_count <= 1,
            "{name} declares `mod common;` {common_count} times; it must be at most once"
        );
        if common_count == 1 {
            any_declares_common = true;
        }

        let ci_sandbox_count = content
            .lines()
            .filter(|l| l.trim() == "mod ci_sandbox_support;")
            .count();
        assert!(
            ci_sandbox_count <= 1,
            "{name} declares `mod ci_sandbox_support;` {ci_sandbox_count} times; it must be at most once"
        );
    }
    assert!(
        any_declares_common,
        "no generated suite declares `mod common;` at all -- since 268 of \
         289 member files use the shared harness, at least one suite must"
    );
}
