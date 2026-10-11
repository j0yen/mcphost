//! PRD-mcphost-timing-assertion-lint
//! AC1 (P0) — Given a scratch tests tree with a file that prints
//! `perf skipped (load)` by hand and asserts `p95 < Duration::from_millis(50)`
//! on a later line (the pre-#176 `docsearch_ac11:53-58` shape), When
//! `scripts/timing-lint.py` runs, Then exit 1 naming that file:line and
//! token; Given the same assertion wrapped in `perf_budget!`, Then exit 0.
//!
//! Scratch-tree convention of `flakelint_ac04`: the real script is copied
//! into a throwaway dir and run over a fixture `tests/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A scratch dir holding `tests/<name>` fixtures; returns the dir.
fn scratch(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-tlint-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    for (name, body) in files {
        let p = dir.join("tests").join(name);
        fs::create_dir_all(p.parent().unwrap()).expect("mkdir fixture dir");
        fs::write(p, body).expect("write fixture");
    }
    dir
}

fn run_lint(dir: &Path) -> (i32, String) {
    let out = Command::new("python3")
        .arg("-I")
        .arg(repo_root().join("scripts/timing-lint.py"))
        .arg("tests")
        .current_dir(dir)
        .output()
        .expect("run python3 scripts/timing-lint.py");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

const HAND_SKIP: &str = r#"
#[test]
fn p95_is_fast() {
    let p95 = measure();
    if std::env::var("MCPHOST_PERF_SKIP").is_ok() {
        eprintln!("perf skipped (load)");
        return;
    }
    assert!(
        p95 < Duration::from_millis(50),
        "p95 too slow"
    );
}
"#;

const WRAPPED: &str = r#"
#[test]
fn p95_is_fast() {
    crate::perf_budget!(50, {
        let p95 = measure();
        assert!(p95 < Duration::from_millis(50), "p95 too slow");
    });
}
"#;

const HELPERS: &str = "/// timing-helper\n#[macro_export]\nmacro_rules! perf_budget {\n    () => {};\n}\n";

#[test]
fn hand_rolled_skip_with_bare_assert_is_reported_with_file_line_and_token() {
    let dir = scratch(
        "ac01-bad",
        &[("docsearch_like.rs", HAND_SKIP), ("common/mod.rs", HELPERS)],
    );
    let (code, stdout) = run_lint(&dir);
    assert_eq!(code, 1, "bare assert must fail the lint; stdout:\n{stdout}");
    assert!(
        stdout.contains("tests/docsearch_like.rs:9: Duration::from_"),
        "report must name file:line and token; stdout:\n{stdout}"
    );
}

#[test]
fn same_assertion_inside_perf_budget_passes() {
    let dir = scratch(
        "ac01-ok",
        &[("docsearch_like.rs", WRAPPED), ("common/mod.rs", HELPERS)],
    );
    let (code, stdout) = run_lint(&dir);
    assert_eq!(code, 0, "wrapped assert must pass; stdout:\n{stdout}");
    assert!(stdout.contains("timing-lint: 0 offenders, 1 helpers"), "{stdout}");
}
