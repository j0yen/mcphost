//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC4 (P0) — Given every Kind, every minimal spec and every removed field, When
//! the message is rendered, Then it is at most 1024 bytes; a kind whose full form
//! exceeds the cap renders the `field` and `docs` clauses without `minimal spec`,
//! and the test names every kind that fell back.

use mcphost::errors::AppError;
use mcphost::kinds::kinddoc::{INVALID_SPEC_MESSAGE_CAP, enrich_invalid_spec, invalid_spec_message};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[test]
fn spmsg_ac04() {
    assert_eq!(INVALID_SPEC_MESSAGE_CAP, 1024);
    let registry = mcphost::kinds::production_registry();
    let mut failures = Vec::new();
    let mut fell_back = BTreeSet::new();
    let mut probes = 0;
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        for spec in kind.minimal_specs() {
            for removed in spec.as_object().expect("a minimal spec is an object").keys() {
                let mut minus = spec.clone();
                minus.as_object_mut().expect("object").remove(removed);
                let Some(err) = AppError::from_kind_violations(kind.validate_all(&minus)) else { continue };
                let AppError::Structured { message, data, .. } = enrich_invalid_spec(kind.as_ref(), err) else {
                    continue;
                };
                probes += 1;
                if message.len() > INVALID_SPEC_MESSAGE_CAP {
                    failures.push(format!("{name} minus `{removed}`: {} bytes", message.len()));
                }
                // A message that carries no minimal spec although the payload has one fell back.
                let has_specs = data["minimal_specs"].as_array().is_some_and(|l| !l.is_empty());
                if has_specs && !message.contains("; minimal spec: ") {
                    fell_back.insert(name.to_string());
                    let docs = data["docs_url"].as_str().unwrap_or_default();
                    if !message.ends_with(&format!("; docs: {docs}")) {
                        failures.push(format!("{name} minus `{removed}`: fallback lost its docs clause"));
                    }
                }
            }
        }
    }
    assert!(probes > 0, "the registry yields probes");
    eprintln!("spmsg_ac04: kinds that fell back to the field + docs form: {fell_back:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The fallback itself, on a payload no real kind produces: a minimal spec too
/// large for the cap is dropped keeping `field` and `docs`; a reason too large
/// is cut with `…`, never the `field` or `docs` clause.
#[test]
fn spmsg_ac04_fallback_forms() {
    let data = |reason: &str, spec_len: usize| -> Value {
        json!({
            "kind": "k", "field": "f", "reason": reason,
            "fields": [{"name": "f", "ty": "string", "required": "yes", "meaning": "m"}],
            "minimal_specs": [{"f": "x".repeat(spec_len)}],
            "docs_url": "https://mcphost.dev/docs/kinds/k",
        })
    };
    let fits = invalid_spec_message(&data("short", 20));
    assert!(fits.contains("; minimal spec: "), "{fits}");

    let dropped = invalid_spec_message(&data("short", 2000));
    assert_eq!(
        dropped,
        "invalid spec: short; field `f` is required always; docs: https://mcphost.dev/docs/kinds/k"
    );

    let long_reason = "é".repeat(3000);
    let cut = invalid_spec_message(&data(&long_reason, 2000));
    assert!(cut.len() <= INVALID_SPEC_MESSAGE_CAP, "{} bytes", cut.len());
    assert!(cut.starts_with("invalid spec: é"), "{cut}");
    assert!(cut.ends_with("…; field `f` is required always; docs: https://mcphost.dev/docs/kinds/k"), "{cut}");
    assert!(!cut.contains("minimal spec"), "{cut}");
}
