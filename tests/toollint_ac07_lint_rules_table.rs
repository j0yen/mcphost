//! PRD-mcphost-publish-time-tool-lint
//! AC7 (P0) -- Given `LINT_RULES`, When the table test runs, Then every row's
//! `test_name` exists, ids are unique snake_case, and every `message`/`fix`
//! template renders with no `{` left for the row's fixture.

use mcphost::kinds::python_lint::{LINT_RULES, lint_source};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn snake_case(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !id.starts_with('_')
        && !id.ends_with('_')
        && !id.contains("__")
        && !id.as_bytes()[0].is_ascii_digit()
}

/// Every `toollint_ac*.rs` integration test file's text, concatenated.
fn toollint_tests() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("toollint_ac") && n.ends_with(".rs"))
        })
        .collect();
    names.sort();
    names.iter().map(|p| std::fs::read_to_string(p).expect("read test file")).collect::<Vec<_>>().join("\n")
}

#[test]
fn every_row_has_a_test_a_unique_snake_case_id_and_renders_fully() {
    assert!(LINT_RULES.len() >= 3, "the three PRD rules are present");
    let tests = toollint_tests();
    let mut ids = BTreeSet::new();
    for rule in LINT_RULES {
        assert!(snake_case(rule.id), "id `{}` must be snake_case", rule.id);
        assert!(ids.insert(rule.id), "duplicate rule id `{}`", rule.id);
        assert!(
            tests.contains(&format!("fn {}(", rule.test_name)),
            "rule `{}` names test `{}`, which no toollint_ac*.rs file defines",
            rule.id,
            rule.test_name
        );

        let findings = lint_source(rule.fixture, None);
        let own: Vec<_> = findings.iter().filter(|f| f["rule"] == rule.id).collect();
        assert!(!own.is_empty(), "rule `{}`'s fixture must trigger it: {findings:?}", rule.id);
        for f in own {
            for field in ["message", "fix"] {
                let text = f[field].as_str().expect("string field");
                assert!(!text.is_empty(), "{}: {field} is empty", rule.id);
                assert!(!text.contains('{'), "{}: {field} left a placeholder: {text}", rule.id);
            }
        }
    }
}

#[test]
fn a_row_without_a_real_test_name_is_refused() {
    // The table check's own negative: a name no file defines is not found.
    let missing = format!("fn {}(", ["no", "such", "toollint", "test"].join("_"));
    assert!(!toollint_tests().contains(&missing));
}
