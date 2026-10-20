//! PRD-mcphost-kind-field-docs-required-from-parse-spec
//! AC2 (P0) — Given every Kind in the registry, When each minimal spec is
//! parsed as-is and then minus each of its own fields, Then every as-is parse
//! succeeds and every minus-one parse fails, with a failure naming the kind,
//! spec index and field; the test iterates the registry and contains no kind
//! name literal.

use mcphost::kinds::kinddoc::minimality_failures;

#[test]
fn kreqd_ac02_minimal_specs_are_minimal() {
    let registry = mcphost::kinds::production_registry();
    let names = registry.names();
    assert!(!names.is_empty(), "the production registry registers kinds");
    let mut failures = Vec::new();
    for name in names {
        let kind = registry.get(name).expect("registered kind resolves");
        failures.extend(minimality_failures(kind.as_ref()));
    }
    assert!(failures.is_empty(), "minimal specs are not minimal:\n{}", failures.join("\n"));
}
