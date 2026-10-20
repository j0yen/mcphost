//! PRD-mcphost-kind-field-docs-required-from-parse-spec
//! AC3 (P0) — Given gendocs runs on the landed tree, When
//! `docs/kinds/<kind>.md` is rendered for every Kind, Then the table between
//! the `kind-fields` markers has a `required` column with `yes` / `no` /
//! ``unless `<fields>` `` matching AC 1's derivation, and the committed file
//! equals the generated output.

use mcphost::kinds::kinddoc::derive_requiredness;
use mcphost::kinds::production_registry;
use mcphost::plans::PlanCatalog;

/// The rows between the `kind-fields` markers, as `(field, required cell)`.
fn field_rows(page: &str) -> Vec<(String, String)> {
    let start = page.find("<!-- kind-fields:start -->").expect("kind-fields start marker");
    let end = page.find("<!-- kind-fields:end -->").expect("kind-fields end marker");
    let mut lines = page[start..end].lines().filter(|l| l.starts_with('|'));
    let header: Vec<&str> = lines.next().expect("header row").split('|').map(str::trim).collect();
    let col = header.iter().position(|h| *h == "required").expect("a `required` column");
    lines
        .skip(1)
        .map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            (cells[1].trim_matches('`').to_string(), cells[col].to_string())
        })
        .collect()
}

#[test]
fn kreqd_ac03_kind_pages_required_column() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let registry = production_registry();
    let plans = PlanCatalog::default_catalog();
    let stale = mcphost::gendocs::run_kinds(root, &registry, &plans, false).expect("gendocs runs");
    assert!(stale.is_empty(), "committed kind pages differ from the generated output: {stale:?}");
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        let page = std::fs::read_to_string(root.join(format!("docs/kinds/{name}.md"))).expect("kind page");
        let rows = field_rows(&page);
        for (doc, req) in derive_requiredness(kind.as_ref()) {
            let cell = rows
                .iter()
                .find(|(f, _)| f == doc.name)
                .unwrap_or_else(|| panic!("kind {name}: no `{}` row in the kind-fields table", doc.name));
            assert_eq!(cell.1, req.cell(), "kind {name}: `{}` row", doc.name);
            let shape_ok = cell.1 == "yes" || cell.1 == "no" || cell.1.starts_with("unless `");
            assert!(shape_ok, "kind {name}: `{}` cell `{}` is not yes / no / unless `<fields>`", doc.name, cell.1);
        }
    }
}
