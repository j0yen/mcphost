//! AC2 (P0) — Given every `AppError` variant from `all_variants_for_test()`,
//! When converted to `ErrorData`, Then each `data.feedback_hint` starts with
//! `host.feedback.send {kind:"` and its kind equals `CODE_TO_KIND[code]`.

use mcphost::errors::all_variants_for_test;
use mcphost::feedback::{CODE_TO_KIND, kind_in};

#[test]
fn every_variant_carries_a_hint_whose_kind_matches_the_table() {
    let variants = all_variants_for_test();
    assert!(variants.len() >= 30, "enumeration looks truncated: {}", variants.len());
    for err in variants {
        let code = err.code();
        let data = err.into_error_data().data.expect("error data");
        let hint = data["feedback_hint"]
            .as_str()
            .unwrap_or_else(|| panic!("{code}: no feedback_hint in {data}"));
        let rest = hint
            .strip_prefix("host.feedback.send {kind:\"")
            .unwrap_or_else(|| panic!("{code}: bad hint prefix: {hint}"));
        let kind = rest.split('"').next().unwrap();
        let want = kind_in(CODE_TO_KIND, code)
            .unwrap_or_else(|| panic!("{code}: not in CODE_TO_KIND"))
            .as_str();
        assert_eq!(kind, want, "{code}: hint kind");
        assert!(hint.contains(&format!("error_code:\"{code}\"")), "{code}: {hint}");
    }
}
