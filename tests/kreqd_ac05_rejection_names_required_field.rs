//! PRD-mcphost-kind-field-docs-required-from-parse-spec
//! AC5 (P0) — Given every Kind, minimal spec and removed field, When
//! `parse_spec` rejects, Then the error text names a field in `field_docs()`
//! whose requiredness is not `Never`.

use mcphost::kinds::kinddoc::{Requiredness, derive_requiredness};

/// True when `name` appears in `text` as a whole identifier, not inside a
/// longer one (`url` in `upstream_url` does not count).
fn names_field(text: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

#[test]
fn kreqd_ac05_rejection_names_required_field() {
    let registry = mcphost::kinds::production_registry();
    let mut failures = Vec::new();
    let mut probes = 0;
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        let required: Vec<&str> = derive_requiredness(kind.as_ref())
            .into_iter()
            .filter(|(_, r)| *r != Requiredness::Never)
            .map(|(d, _)| d.name)
            .collect();
        for (i, spec) in kind.minimal_specs().iter().enumerate() {
            for field in spec.as_object().expect("a minimal spec is an object").keys() {
                let mut minus = spec.clone();
                minus.as_object_mut().expect("object").remove(field);
                probes += 1;
                match kind.validate(&minus) {
                    Ok(()) => failures.push(format!("kind {name} spec #{i} minus `{field}`: accepted")),
                    Err(e) => {
                        let text = e.to_string();
                        if !required.iter().any(|r| names_field(&text, r)) {
                            failures.push(format!(
                                "kind {name} spec #{i} minus `{field}`: error `{text}` names none of {required:?}"
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(probes > 0, "the registry yields probes");
    assert!(failures.is_empty(), "rejections that do not name a required field:\n{}", failures.join("\n"));
}
