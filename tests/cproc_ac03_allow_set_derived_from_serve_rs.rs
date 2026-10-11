//! PRD-mcphost-child-process-test-harness AC3 — the lint's allowed call set
//! is the `pub fn` names of `tests/support/serve.rs`: a new `pub fn` there is
//! accepted with no edit to the script, and the script's source names no
//! function that `serve.rs` exports.

use crate::child_proc_lint;

use child_proc_lint::{fixture, repo_root, run, scratch, write};
use std::fs;

#[test]
fn a_new_pub_fn_in_serve_rs_is_accepted_without_a_script_edit() {
    let dir = scratch("ac03");
    write(&dir, "uses_new_helper.rs", &fixture("uses_new_helper.rs.tpl"));

    // Not yet exported by serve.rs: the call is flagged.
    let (code, out) = run(&dir);
    assert_eq!(code, 1, "undeclared helper must be flagged:\n{out}");
    assert!(out.contains("uses_new_helper.rs:4:"), "{out}");

    // serve.rs gains the pub fn; the script is untouched.
    let serve = dir.join("tests/support/serve.rs");
    let mut text = fs::read_to_string(&serve).unwrap();
    text.push_str("\nimpl ServeChild {\n    pub fn brand_new_helper() {}\n}\n");
    fs::write(&serve, text).unwrap();
    let (code, out) = run(&dir);
    assert_eq!(code, 0, "new pub fn must be accepted:\n{out}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_script_source_contains_no_function_name_from_serve_rs() {
    let serve = fs::read_to_string(repo_root().join("tests/support/serve.rs")).unwrap();
    let script = fs::read_to_string(child_proc_lint::script()).unwrap();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let names: Vec<&str> = serve
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("pub fn ").or_else(|| l.trim_start().strip_prefix("pub async fn ")))
        .map(|rest| rest.split(|c: char| !is_ident(c)).next().unwrap())
        .collect();
    assert!(names.len() >= 8, "expected serve.rs to export its helpers, got {names:?}");
    for name in names {
        let found = script.match_indices(name).any(|(at, _)| {
            let before = script[..at].chars().next_back();
            let after = script[at + name.len()..].chars().next();
            !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
        });
        assert!(!found, "scripts/child-proc-lint.py names serve.rs fn `{name}`");
    }
}
