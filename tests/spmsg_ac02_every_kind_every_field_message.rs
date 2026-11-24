//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC2 (P0) — Given every Kind, every `minimal_specs()` entry and every field
//! removed, When publish validation refuses, Then the message names `data.field`
//! (which may differ from the removed field) with its `fields` row phrase per R1,
//! the text between `; minimal spec: ` and `; docs: ` when present parses to the
//! first `minimal_specs` entry containing `data.field`, and the test has no kind
//! or field literal.

use mcphost::errors::AppError;
use mcphost::kinds::kinddoc::enrich_invalid_spec;
use serde_json::Value;

#[test]
fn spmsg_ac02() {
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
                let at = format!("{name} #{i} minus `{removed}`");
                let Some(err) = AppError::from_kind_violations(kind.validate_all(&minus)) else {
                    failures.push(format!("{at}: accepted"));
                    continue;
                };
                let AppError::Structured { message, data, .. } = enrich_invalid_spec(kind.as_ref(), err) else {
                    failures.push(format!("{at}: not a structured refusal"));
                    continue;
                };
                let reason = data["reason"].as_str().unwrap_or_default();
                if !message.starts_with(&format!("invalid spec: {reason}")) {
                    failures.push(format!("{at}: head changed: {message}"));
                }
                // The field clause: present exactly when the `fields` row says the field is required.
                let field = data["field"].as_str();
                let cell = field.and_then(|f| {
                    data["fields"].as_array()?.iter().find(|r| r["name"].as_str() == Some(f))?["required"]
                        .as_str()
                        .map(str::to_string)
                });
                let want = match (field, cell.as_deref()) {
                    (Some(f), Some("yes")) => Some(format!("field `{f}` is required always")),
                    (Some(f), Some(c)) if c.starts_with("unless ") => Some(format!("field `{f}` is required {c} is set")),
                    _ => None,
                };
                match &want {
                    Some(clause) if !message.contains(&format!("; {clause}")) => {
                        failures.push(format!("{at}: no `{clause}` in {message}"))
                    }
                    None if message.contains("is required") => {
                        failures.push(format!("{at}: field clause for a non-required row: {message}"))
                    }
                    _ => {}
                }
                // The minimal-spec clause parses to the first entry that contains data.field.
                if let Some((_, tail)) = message.split_once("; minimal spec: ") {
                    let json = tail.rsplit_once("; docs: ").map_or(tail, |(j, _)| j);
                    let specs = data["minimal_specs"].as_array().cloned().unwrap_or_default();
                    let expect: Option<&Value> = field
                        .and_then(|f| specs.iter().find(|s| s.get(f).is_some()))
                        .or_else(|| specs.first());
                    match serde_json::from_str::<Value>(json) {
                        Ok(parsed) if Some(&parsed) == expect => {}
                        other => failures.push(format!("{at}: minimal spec {other:?} != {expect:?}")),
                    }
                }
                if data["docs_url"].as_str().is_some_and(|u| !message.ends_with(&format!("; docs: {u}"))) {
                    failures.push(format!("{at}: docs clause missing: {message}"));
                }
            }
        }
    }
    assert!(probes > 0, "the registry yields probes");
    assert!(failures.is_empty(), "{} of {probes} probes failed:\n{}", failures.len(), failures.join("\n"));
}
