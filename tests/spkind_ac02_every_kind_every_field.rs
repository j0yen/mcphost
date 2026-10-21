//! PRD-mcphost-invalid-spec-error-carries-kind-fields
//! AC2 (P0) — Given every Kind in the registry, every minimal spec and every
//! field removed, When publish validation runs, Then `data.field` equals the
//! removed field, its `fields` row is not `no`, and a minimal spec containing
//! it is present; the test contains no kind or field literal.

use mcphost::errors::AppError;
use mcphost::kinds::kinddoc::enrich_invalid_spec;
use serde_json::Value;

#[test]
fn spkind_ac02_every_kind_every_field() {
    let registry = mcphost::kinds::production_registry();
    let mut failures = Vec::new();
    let mut probes = 0;
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        for (i, spec) in kind.minimal_specs().iter().enumerate() {
            for removed in spec.as_object().expect("a minimal spec is an object").keys() {
                let mut minus = spec.clone();
                minus.as_object_mut().expect("object").remove(removed);
                probes += 1;
                // The publish path: `control::tool_publish` runs exactly this.
                let Some(err) = AppError::from_kind_violations(kind.validate_all(&minus)) else {
                    failures.push(format!("{name} #{i} minus `{removed}`: accepted"));
                    continue;
                };
                let AppError::Structured { code, data, .. } = enrich_invalid_spec(kind.as_ref(), err) else {
                    failures.push(format!("{name} #{i} minus `{removed}`: not a structured refusal"));
                    continue;
                };
                if code != "invalid_spec" {
                    failures.push(format!("{name} #{i} minus `{removed}`: code {code}"));
                    continue;
                }
                let rows = data["fields"].as_array().cloned().unwrap_or_default();
                let row_of = |f: &str| rows.iter().find(|r| r["name"].as_str() == Some(f));
                let reported = data["field"].as_str().unwrap_or_default();
                // Removing the only field of an alternative shape leaves a spec
                // the validator can only call incomplete; there it names a
                // sibling alternative, which the removed field's row lists as
                // an `unless` alternative (or vice versa).
                let alternative = |a: &str, b: &str| {
                    row_of(a).and_then(|r| r["required"].as_str()).is_some_and(|c| c.contains(&format!("`{b}`")))
                };
                if reported != removed && !alternative(reported, removed) && !alternative(removed, reported) {
                    failures.push(format!("{name} #{i} minus `{removed}`: data.field {}", data["field"]));
                    continue;
                }
                let row = row_of(removed);
                if !row.is_some_and(|r| r["required"].as_str().is_some_and(|c| c != "no")) {
                    failures.push(format!("{name} #{i} minus `{removed}`: fields row {row:?}"));
                }
                let has_spec = data["minimal_specs"]
                    .as_array()
                    .is_some_and(|l| l.iter().any(|s: &Value| s.get(removed).is_some()));
                if !has_spec {
                    failures.push(format!("{name} #{i} minus `{removed}`: no minimal spec carries it"));
                }
            }
        }
    }
    assert!(probes > 0, "the registry yields probes");
    assert!(failures.is_empty(), "{} of {probes} probes failed:\n{}", failures.len(), failures.join("\n"));
}
