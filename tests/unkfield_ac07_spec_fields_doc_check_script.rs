//! PRD-mcphost-spec-unknown-field-rejection
//! AC7 — Given the registry's kinds at the landing commit, When their pages
//! are checked, Then every spec field a parser reads appears on its page
//! and every top-level key of a kind's example is one of those fields;
//! given an example with an extra field, Then the check names it.
//!
//! The regex script this AC began as is gone
//! (PRD-mcphost-kind-docs-from-registry-b-v2 R3): the same two checks run
//! over `KindDoc` (`example_keys_outside_spec_fields`) and the generated
//! `kind-fields` block (`gen-docs kinds --check`).

use mcphost::kinds::kinddoc::KindDoc;
use mcphost::plans::PlanCatalog;

fn docs() -> Vec<KindDoc> {
    let registry = mcphost::kinds::production_registry();
    let plans = PlanCatalog::default_catalog();
    registry.names().into_iter().filter_map(|n| registry.get(n)).map(|k| k.doc(&plans)).collect()
}

#[test]
fn landing_commit_is_in_sync_for_every_kind() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let registry = mcphost::kinds::production_registry();
    let plans = PlanCatalog::default_catalog();
    let stale = mcphost::gendocs::run_kinds(root, &registry, &plans, false).expect("check the pages");
    assert!(stale.is_empty(), "kind pages differ from the registry: {stale:?}");
    let docs = docs();
    for kind in ["chain", "echo", "http", "jq", "js", "python", "wasm"] {
        let doc = docs.iter().find(|d| d.name == kind).unwrap_or_else(|| panic!("no KindDoc for {kind}"));
        assert_eq!(doc.example_keys_outside_spec_fields(), Vec::<String>::new(), "{kind}: example keys outside spec_fields");
        let page = std::fs::read_to_string(root.join(format!("docs/kinds/{kind}.md"))).expect("kind page");
        for field in &doc.spec_fields {
            assert!(page.contains(&format!("| `{}` |", field.name)), "{kind}'s parser reads `{}`, its page never lists it", field.name);
        }
    }
}

#[test]
fn a_doc_with_an_extra_field_fails_naming_it() {
    let mut docs = docs();
    let echo = docs.iter_mut().find(|d| d.name == "echo").expect("echo kind");
    echo.example.as_object_mut().expect("echo example is an object").insert("bogus_extra_field".into(), 1.into());
    assert_eq!(echo.example_keys_outside_spec_fields(), vec!["bogus_extra_field".to_string()]);
}
