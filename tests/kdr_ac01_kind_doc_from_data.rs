//! PRD-mcphost-kind-docs-from-registry AC1: `KindDoc` is built from Rust
//! data for every registered kind, with nothing left empty.

use mcphost::kinds::production_registry;
use mcphost::plans::PlanCatalog;

#[test]
fn kdr_ac01_kind_doc_from_data() {
    let registry = production_registry();
    let plans = PlanCatalog::default_catalog();
    assert_eq!(registry.names(), ["chain", "echo", "http", "js", "python", "wasm"]);
    for name in registry.names() {
        let kind = registry.get(name).expect("registered");
        let doc = kind.doc(&plans);
        println!(
            "kind={} fields={} ceilings={} errors={}",
            name,
            doc.spec_fields.len(),
            doc.ceilings.len(),
            doc.error_codes.len()
        );
        assert_eq!(doc.name, name);
        assert!(!doc.summary.is_empty(), "kind={name}: field `summary` is empty");
        assert!(!doc.spec_fields.is_empty(), "kind={name}: field `spec_fields` is empty");
        assert!(!doc.universal_fields.is_empty(), "kind={name}: field `universal_fields` is empty");
        assert!(!doc.example.is_null(), "kind={name}: field `example` is empty");
        for f in &doc.spec_fields {
            assert!(!f.ty.is_empty(), "kind={name}: spec_fields `{}` has no type", f.name);
            assert!(!f.meaning.is_empty(), "kind={name}: spec_fields `{}` has no meaning", f.name);
        }
        for e in &doc.error_codes {
            assert!(!e.when.is_empty(), "kind={name}: error_codes `{}` has no `when`", e.code);
        }
        let has_required = doc.spec_fields.iter().any(|f| f.required);
        assert_eq!(has_required, kind.required_spec_field().is_some(), "kind={name}: required flag");
        // Only the source kinds (python, js) carry bridge modules.
        assert_eq!(doc.bridge_modules.is_some(), name == "python" || name == "js", "kind={name}: bridge_modules");
        if name != "echo" {
            assert!(!doc.ceilings.is_empty(), "kind={name}: field `ceilings` is empty");
        }
        if name != "echo" {
            assert!(!doc.error_codes.is_empty(), "kind={name}: field `error_codes` is empty");
        }
    }
    let python = registry.get("python").unwrap().doc(&plans);
    let names: Vec<_> = python.bridge_modules.unwrap().into_iter().map(|b| b.name).collect();
    let expected: Vec<_> = mcphost::kinds::python::BRIDGE_MODULES.iter().map(|b| b.name.to_string()).collect();
    assert_eq!(names, expected);
}
