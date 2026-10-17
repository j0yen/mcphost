//! AC4 (PRD-mcphost-plugin-skill-generated-from-tools-list) -- Given
//! `AppError` gains a variant with a new `code`, When `cargo test` runs, Then
//! the remedy test fails naming the code until `remedy()` covers it, and
//! every remedy's named verb exists in `host_tools`.
//!
//! A new variant must join `errors::all_variants_for_test` (its exhaustive
//! match refuses to compile otherwise), and `errors::error_codes()` reads that
//! enumeration, so the walk below sees the new code with no edit here.

use std::collections::BTreeSet;

use mcphost::errors::{error_codes, remedy};
use mcphost::kinds::KindRegistry;
use mcphost::skill_gen::{self, Section};

/// Every `host.*` / `billing.*` name written in backticks in `remedy`.
fn named_verbs(remedy: &str) -> Vec<String> {
    remedy
        .split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, tok)| tok.split('(').next().unwrap_or(tok).to_string())
        .filter(|tok| tok.starts_with("host.") || tok.starts_with("billing."))
        .collect()
}

#[test]
fn skgn_ac04() {
    let kinds = KindRegistry::with_builtin();
    let verbs: BTreeSet<String> = skill_gen::registry_verbs(&kinds).into_iter().map(|v| v.name).collect();
    assert!(verbs.contains("host.whoami"), "precondition: the registry is not empty");

    let codes = error_codes();
    assert!(codes.contains(&"tenant_key_missing") && codes.contains(&"args_invalid"));
    let uncovered: Vec<&str> = codes.iter().copied().filter(|c| remedy(c).is_empty()).collect();
    assert!(uncovered.is_empty(), "remedy() has no entry for error code(s): {uncovered:?}");

    for code in &codes {
        for verb in named_verbs(remedy(code)) {
            assert!(verbs.contains(&verb), "remedy for `{code}` names `{verb}`, which host_tools does not serve");
        }
    }

    // A code nobody wrote a remedy for stays empty (what the walk above refuses).
    assert_eq!(remedy("a_code_no_variant_has"), "");

    // The `errors` section carries each code with its remedy, from the same call.
    let section = skill_gen::render_section(Section::Errors, &skill_gen::Sources::from_registry(&kinds));
    for code in &codes {
        let row = format!("| `{code}` | {} |", remedy(code));
        assert!(section.contains(&row), "errors section is missing the row for `{code}`");
    }
}
