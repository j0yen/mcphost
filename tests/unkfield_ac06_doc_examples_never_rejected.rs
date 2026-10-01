//! AC6 (P0, guardrail) — Given the example spec from every
//! `docs/kinds/*.md` page (the same content `host.quickstart` serves per
//! kind -- both read `Kind::example()`, which is exactly
//! `kinds::docs::parse_kind_doc` on these same files), When each is
//! checked, Then none fails `unknown_spec_field`.
//!
//! This is the requirement 1 guardrail metric made concrete: the new
//! unknown-field check must never reject a spec this host's own docs
//! teach an agent to write. Runs `Kind::validate_all` directly (not
//! through a real `host.tool_publish`/`host.spec_test` RPC) so this test
//! needs neither a running server nor sandbox user namespaces -- it is a
//! pure function of each kind's own parser, which is exactly what
//! `check_unknown_spec_fields` (the thing being guarded against here) runs
//! inside.

use crate::common;
use common::{TempDataDir, all_kinds_registry, chain_kind_registry};
use mcphost::kinds::Kind;
use mcphost::kinds::docs::parse_kind_doc;
use mcphost::kinds::wasm::WasmKind;

fn assert_never_unknown_spec_field(kind_name: &str, errors: &[mcphost::kinds::KindError]) {
    for e in errors {
        if let mcphost::kinds::KindError::Structured { code, data, .. } = e {
            assert_ne!(
                *code,
                "unknown_spec_field",
                "{kind_name}'s own doc example must never be rejected as an unknown spec \
                 field: {data:?}"
            );
        }
    }
}

#[test]
fn echo_and_http_doc_examples_never_trip_unknown_spec_field() {
    let envs_dir = TempDataDir::new();
    let kinds = all_kinds_registry(&envs_dir.0);

    for (name, markdown) in [
        ("echo", include_str!("../docs/kinds/echo.md")),
        ("http", include_str!("../docs/kinds/http.md")),
    ] {
        let kind = kinds.get(name).unwrap_or_else(|| panic!("{name} is registered"));
        let example = parse_kind_doc(markdown);
        let errors = kind.validate_all(&example.spec);
        assert_never_unknown_spec_field(name, &errors);
    }
}

/// `python`'s own doc example, checked the same way -- `validate_all`
/// alone (no `validate_async`, which is what actually needs a sandboxed
/// subprocess to AST-check `source`), so this needs no user namespaces.
#[test]
fn python_doc_example_never_trips_unknown_spec_field() {
    let envs_dir = TempDataDir::new();
    let kinds = all_kinds_registry(&envs_dir.0);
    let kind = kinds.get("python").expect("python is registered");
    let example = parse_kind_doc(include_str!("../docs/kinds/python.md"));
    let errors = kind.validate_all(&example.spec);
    assert_never_unknown_spec_field("python", &errors);
}

/// `wasm`'s doc example is a placeholder base64 blob (the doc says so
/// itself: "the shape is what matters") that may well fail to compile as
/// a real component -- this only asserts that if it fails, it's never for
/// carrying a field the parser doesn't read.
#[test]
fn wasm_doc_example_never_trips_unknown_spec_field() {
    let kind = WasmKind::new();
    let example = parse_kind_doc(include_str!("../docs/kinds/wasm.md"));
    let errors = kind.validate_all(&example.spec);
    assert_never_unknown_spec_field("wasm", &errors);
}

/// `chain` has no `docs/kinds/chain.md` entry in `kinds::docs::KIND_DOCS`
/// (it predates this file's doc-driven `Kind::example()` convention), but
/// its own doc's worked example is still checked directly here for the
/// same guardrail.
#[test]
fn chain_doc_example_never_trips_unknown_spec_field() {
    let kinds = chain_kind_registry();
    let kind = kinds.get("chain").expect("chain is registered");
    let example = parse_kind_doc(include_str!("../docs/kinds/chain.md"));
    let errors = kind.validate_all(&example.spec);
    assert_never_unknown_spec_field("chain", &errors);
}
