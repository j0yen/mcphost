//! AC3 (P0) — Given a code with no mapping (the test removes
//! `tool_not_found` from a copy of the table), When
//! `fbeh_every_code_has_a_kind` runs, Then it fails naming `tool_not_found`.

use mcphost::errors::all_variants_for_test;
use mcphost::feedback::{CODE_TO_KIND, FeedbackKind, KIND_LEVEL_CODES, check_every_code_has_a_kind};

fn every_code() -> Vec<&'static str> {
    let mut codes: Vec<&'static str> =
        all_variants_for_test().iter().map(|e| e.code()).collect();
    codes.extend(KIND_LEVEL_CODES);
    codes.sort_unstable();
    codes.dedup();
    codes
}

#[test]
fn fbeh_every_code_has_a_kind() {
    check_every_code_has_a_kind(CODE_TO_KIND, &every_code()).unwrap();
}

#[test]
fn a_table_missing_tool_not_found_fails_naming_it() {
    let copy: Vec<(&str, FeedbackKind)> = CODE_TO_KIND
        .iter()
        .copied()
        .filter(|(c, _)| *c != "tool_not_found")
        .collect();
    let err = check_every_code_has_a_kind(&copy, &every_code()).unwrap_err();
    assert!(err.contains("tool_not_found"), "{err}");
    assert_eq!(err, "feedback kind mapping missing for tool_not_found");
}

#[test]
fn a_table_key_no_code_produces_fails() {
    let mut copy: Vec<(&str, FeedbackKind)> = CODE_TO_KIND.to_vec();
    copy.push(("quota_exceeded_ghost", FeedbackKind::Bug));
    let err = check_every_code_has_a_kind(&copy, &every_code()).unwrap_err();
    assert!(err.contains("quota_exceeded_ghost"), "{err}");
}
