//! PRD-mcphost-timing-assertion-lint
//! AC2 (P0) — Given a scratch file whose `perf_budget!` block is followed by a separate
//! bare `assert!(t.elapsed() < Duration::from_millis(5))`, When the script runs,
//! Then it names the bare line (the file-level `src.contains("perf_budget!")`
//! check in `flakelint_ac07` passed this file).


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

const HELPERS: &str = "/// timing-helper\n#[macro_export]\nmacro_rules! perf_budget {\n    () => {};\n}\n";

const MIXED: &str = r#"
#[test]
fn mixed() {
    crate::perf_budget!(50, {
        assert!(t.elapsed() < Duration::from_millis(50));
    });
    let t = Instant::now();
    assert!(t.elapsed() < Duration::from_millis(5));
}
"#;

#[test]
fn bare_assert_after_a_perf_budget_block_is_named_and_the_wrapped_one_is_not() {
    let dir = scratch("ac02", &[("mixed.rs", MIXED), ("common/mod.rs", HELPERS)]);
    let (code, stdout) = run_lint(&dir);
    assert_eq!(code, 1, "stdout:\n{stdout}");
    assert!(stdout.contains("tests/mixed.rs:8: elapsed("), "{stdout}");
    assert!(!stdout.contains("tests/mixed.rs:5:"), "wrapped line flagged: {stdout}");
    assert!(stdout.contains("timing-lint: 1 offenders"), "{stdout}");
}
