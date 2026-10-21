//! PRD-mcphost-invalid-spec-error-carries-kind-fields
//! AC4 (P0) — Given every Kind, When its `fields` plus `minimal_specs`
//! payload is serialized, Then it is ≤ 8 KB, or the kind falls back to
//! `docs_url` plus the single `field` row and the test names the kind that
//! fell back.

use mcphost::kinds::kinddoc::{INVALID_SPEC_PAYLOAD_CAP, fit_payload, invalid_spec_data};
use serde_json::{Value, json};

fn size(data: &Value) -> usize {
    serde_json::to_vec(&(&data["fields"], &data["minimal_specs"])).unwrap().len()
}

#[test]
fn spkind_ac04_payload_size_cap() {
    let registry = mcphost::kinds::production_registry();
    let mut fell_back = Vec::new();
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        let named = kind.known_spec_fields().first().copied();
        let data = invalid_spec_data(kind.as_ref(), named, "probe");
        assert!(data["docs_url"].as_str().is_some_and(|u| u.ends_with(name)), "{name}: docs_url");
        if size(&data) <= INVALID_SPEC_PAYLOAD_CAP {
            continue;
        }
        fell_back.push(name.to_string());
        assert!(
            data["fields"].as_array().is_some_and(|r| r.len() <= 1) && data["minimal_specs"] == json!([]),
            "kind {name} fell back but still ships the full payload"
        );
    }
    eprintln!("kinds that fell back to docs_url + the single field row: {fell_back:?}");
}

#[test]
fn spkind_ac04_oversized_payload_falls_back_to_the_single_field_row() {
    let rows: Vec<Value> = (0..400).map(|i| json!({"name": format!("f{i}"), "meaning": "x".repeat(40)})).collect();
    let specs = vec![json!({"f7": 1})];
    let (kept, dropped) = fit_payload(rows.clone(), specs.clone(), Some("f7"));
    assert_eq!(kept, vec![json!({"name": "f7", "meaning": "x".repeat(40)})]);
    assert!(dropped.is_empty());
    let (all, all_specs) = fit_payload(rows[..3].to_vec(), specs.clone(), Some("f1"));
    assert_eq!((all.len(), all_specs), (3, specs));
}
