//! PRD-mcphost-error-codes-generated-registry
//! AC1 (P0) — Given a code declared with `err_code!` in the module that
//! raises it, When the crate builds, Then `lookup` returns an entry whose
//! code, family, class, retryable and remedy equal the declaration, the raise
//! site compiles against the declared const, and no `row(` line for that code
//! exists in the hand table.

use mcphost::error_registry::{ErrorClass, Family, lookup};
use mcphost::{resources, sampling};

#[test]
fn errreg_ac01_declaration_is_the_registry_row() {
    // The raise-site const IS the declared code: this compiles only against
    // the const `err_code!` defined.
    let e = lookup(sampling::SAMPLING_TIMEOUT).expect("declared code is registered");
    assert_eq!(e.code, "sampling_timeout");
    assert_eq!(e.family, Family::Call);
    assert_eq!(e.class, ErrorClass::AgentFault);
    assert!(e.retryable);
    assert!(e.remedy.starts_with("Retry with a smaller max_tokens"), "{}", e.remedy);
    assert!(e.site.starts_with("src/sampling.rs:"), "site {:?}", e.site);
    let _ = resources::RESOURCES_MAX;
}

#[test]
fn errreg_ac01_declared_codes_have_no_hand_table_row() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let table = std::fs::read_to_string(root.join("src/error_registry.rs")).expect("read");
    for code in [resources::RESOURCES_MAX, sampling::SAMPLING_TIMEOUT, sampling::SAMPLING_UNSUPPORTED] {
        let needle = format!("row(\"{code}\"");
        assert!(
            !table.lines().any(|l| l.trim_start().starts_with(&needle)),
            "{code} still has a hand-table row"
        );
        assert!(lookup(code).is_some(), "{code} not registered by its declaration");
    }
}
