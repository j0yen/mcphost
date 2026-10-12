//! PRD-mcphost-timing-assertion-lint
//! AC3 (P0) — Given a scratch `tests/common/mod.rs` exporting `macro_rules! my_budget`
//! under a `/// timing-helper` doc line, When a test asserts elapsed inside
//! `my_budget!{…}`, Then exit 0; Given the doc line removed, Then exit 1 — the
//! allow set comes from the harness file, and the script contains no helper
//! name.


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

const USER: &str = r#"
#[test]
fn t() {
    my_budget! {
        assert!(t.elapsed() < Duration::from_millis(5));
    }
}
"#;

const MARKED: &str = "/// timing-helper\nmacro_rules! my_budget {\n    () => {};\n}\n";
const UNMARKED: &str = "/// not a helper\nmacro_rules! my_budget {\n    () => {};\n}\n";

#[test]
fn marked_helper_in_harness_file_sanctions_its_body() {
    let dir = scratch("ac03-ok", &[("user.rs", USER), ("common/mod.rs", MARKED)]);
    let (code, stdout) = run_lint(&dir);
    assert_eq!(code, 0, "stdout:\n{stdout}");
    assert!(stdout.contains("0 offenders, 1 helpers"), "{stdout}");
}

#[test]
fn removing_the_doc_line_makes_the_same_file_an_offender() {
    let dir = scratch("ac03-bad", &[("user.rs", USER), ("common/mod.rs", UNMARKED)]);
    let (code, stdout) = run_lint(&dir);
    assert_eq!(code, 1, "stdout:\n{stdout}");
    assert!(stdout.contains("tests/user.rs:5: elapsed("), "{stdout}");
}

#[test]
fn script_names_no_helper() {
    let script = fs::read_to_string(repo_root().join("scripts/timing-lint.py")).unwrap();
    for name in ["perf_budget", "perf_overhead_budget", "deadline_budget", "serve_ready_timeout", "my_budget"] {
        assert!(!script.contains(name), "scripts/timing-lint.py hard-codes `{name}`");
    }
}
