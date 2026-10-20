//! PRD-mcphost-kind-field-docs-required-from-parse-spec
//! AC4 (P0) — Given a planted edit that changes the http `method` row to
//! `no`, When the kreqd drift test runs, Then it fails naming `http` and the
//! `method` row.

use mcphost::gendocs::kind_fields_drift;
use mcphost::kinds::production_registry;
use mcphost::plans::PlanCatalog;

fn page(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("docs/kinds/{name}.md")))
        .expect("kind page")
}

#[test]
fn kreqd_ac04_planted_method_row_drift() {
    let registry = production_registry();
    let plans = PlanCatalog::default_catalog();
    let kind = registry.get("http").expect("http kind registered");
    let doc = kind.doc(&plans);
    let committed = page("http");
    assert_eq!(kind_fields_drift("http", &doc, &committed), None, "the committed http page is in step");

    let row = committed
        .lines()
        .find(|l| l.starts_with("| `method` |"))
        .expect("an http `method` row");
    let planted_row = "| `method` | string | no | HTTP method of the upstream request |";
    assert_ne!(row, planted_row, "the plant must change the row");
    let planted = committed.replace(row, planted_row);
    let err = kind_fields_drift("http", &doc, &planted).expect("a planted edit must fail the drift check");
    assert!(err.contains("http"), "must name the kind: {err}");
    assert!(err.contains("`method`"), "must name the method row: {err}");
}
