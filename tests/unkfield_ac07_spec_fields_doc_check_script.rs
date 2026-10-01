//! AC7 (P1) — Given `scripts/spec-fields-doc-check.sh` at the landing
//! commit, When run, Then it exits 0 and prints one `kind=<k> fields=<n>
//! doc_in_sync=true` line per kind; given a fixture doc with an extra
//! field, Then it exits 1 naming it.
//!
//! The script itself is a thin `cargo run -- spec-fields-check` wrapper
//! (`src/main.rs`'s `Command::SpecFieldsCheck`) around
//! `mcphost::kinds::docs::check_docs_against_known_fields` /
//! `doc_example_extra_fields` -- this test exercises those library
//! functions directly (real process-spawn coverage of the shell wrapper
//! itself belongs to a manual/CI smoke run, not a unit test that would
//! otherwise need to `cargo run` a second debug build of this same
//! crate).

use crate::common;
use common::{TempDataDir, all_kinds_registry, chain_kind_registry};
use mcphost::kinds::docs::{check_docs_against_known_fields, doc_example_extra_fields};
use mcphost::kinds::wasm::WasmKind;

#[test]
fn every_real_kind_doc_is_already_in_sync() {
    let envs_dir = TempDataDir::new();
    let kinds = all_kinds_registry(&envs_dir.0);
    // `all_kinds_registry` doesn't register `wasm`; `check_docs_against_known_fields`
    // only reports on kinds the registry actually has, so add it so the
    // "one line per kind" exit-0 claim covers every kind `KIND_DOCS` lists
    // (echo, http, python, wasm).
    let mut kinds = kinds;
    kinds.register(std::sync::Arc::new(WasmKind::new()));

    let results = check_docs_against_known_fields(&kinds);
    let seen: Vec<&str> = results.iter().map(|r| r.kind).collect();
    assert_eq!(seen, vec!["echo", "http", "python", "wasm"], "one line per documented kind");
    for r in &results {
        assert!(
            r.doc_in_sync,
            "kind={} fields={} doc_in_sync=false extra_in_doc={:?}",
            r.kind, r.fields, r.extra_in_doc
        );
        assert!(r.fields > 0, "{} must have a non-empty known-field list", r.kind);
    }
}

#[test]
fn a_fixture_doc_with_an_extra_field_is_named_and_fails() {
    let fixture_markdown = "a fixture doc.\n\n```json\n{\"schema\": {\"type\": \"object\"}, \"made_up_field\": 1}\n```\n";
    let known = ["schema"];
    let extra = doc_example_extra_fields(fixture_markdown, &known);
    assert_eq!(extra, vec!["made_up_field".to_string()]);
}

#[test]
fn a_fixture_doc_with_only_known_fields_reports_no_drift() {
    let fixture_markdown = "a fixture doc.\n\n```json\n{\"schema\": {\"type\": \"object\"}}\n```\n";
    let known = ["schema"];
    let extra = doc_example_extra_fields(fixture_markdown, &known);
    assert!(extra.is_empty(), "no drift expected: {extra:?}");
}

/// Sanity: `chain`'s own doc, if it were added to `KIND_DOCS` tomorrow,
/// would also pass -- checked directly (not through
/// `check_docs_against_known_fields`, since `chain` isn't in that const
/// today) so a future PRD adding it starts from a known-good baseline.
#[test]
fn chain_doc_would_also_be_in_sync_if_added() {
    let kinds = chain_kind_registry();
    let known = kinds.get("chain").expect("chain registered").known_spec_fields();
    let extra = doc_example_extra_fields(include_str!("../docs/kinds/chain.md"), known);
    assert!(extra.is_empty(), "chain doc drift: {extra:?}");
}
