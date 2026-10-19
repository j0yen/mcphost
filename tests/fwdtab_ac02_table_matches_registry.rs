//! PRD-mcphost-generated-dispatch-table-stack-budget
//! AC2 — Given the table and `host_tool_descriptors(&KindRegistry::with_builtin())`,
//! When `fwdtab_ac02` compares the table's names with the registry's canonical
//! dotted `host.*` names, Then the sets are equal; and given one descriptor
//! temporarily removed from the registry in the test, Then the test fails naming
//! that verb as table-only (and a removed row fails naming it as registry-only).
//! contract: reads registry handler::host_tool_descriptors

use mcphost::dispatch_table;
use mcphost::kinds::KindRegistry;
use std::collections::BTreeSet;

/// The canonical dotted `host.*` names of the registry (aliases excluded), the
/// walk `verbforward::resolve` and the stack-budget guard do.
fn registry_names(skip: Option<&str>) -> BTreeSet<String> {
    let kinds = KindRegistry::with_builtin();
    mcphost::handler::host_tool_descriptors(&kinds)
        .into_iter()
        .filter(|t| t.name.starts_with("host.") && mcphost::tool_aliases::resolve(&t.name).is_none())
        .map(|t| t.name.to_string())
        .filter(|n| Some(n.as_str()) != skip)
        .collect()
}

fn table_names(skip: Option<&str>) -> BTreeSet<String> {
    dispatch_table::names()
        .into_iter()
        .filter(|n| Some(*n) != skip)
        .map(str::to_string)
        .collect()
}

/// `Err` names every verb present on one side only.
fn drift(table: &BTreeSet<String>, registry: &BTreeSet<String>) -> Result<(), String> {
    let table_only: Vec<&String> = table.difference(registry).collect();
    let registry_only: Vec<&String> = registry.difference(table).collect();
    if table_only.is_empty() && registry_only.is_empty() {
        return Ok(());
    }
    Err(format!("table-only: {table_only:?}; registry-only: {registry_only:?}"))
}

#[test]
fn fwdtab_ac02_table_and_registry_name_the_same_verbs() {
    if let Err(why) = drift(&table_names(None), &registry_names(None)) {
        panic!("src/dispatch_table.rs and host_tool_descriptors disagree ({why}): run scripts/gen-dispatch-table.sh --write");
    }
    assert!(table_names(None).len() > 200, "the table is not empty");
}

#[test]
fn fwdtab_ac02_descriptor_removed_from_registry_fails_naming_table_only() {
    let why = drift(&table_names(None), &registry_names(Some("host.whoami"))).expect_err("a missing descriptor is drift");
    assert!(why.contains("table-only: [\"host.whoami\"]"), "{why}");
    assert!(why.contains("registry-only: []"), "{why}");
}

#[test]
fn fwdtab_ac02_row_removed_from_table_fails_naming_registry_only() {
    let why = drift(&table_names(Some("host.whoami")), &registry_names(None)).expect_err("a missing row is drift");
    assert!(why.contains("registry-only: [\"host.whoami\"]"), "{why}");
    assert!(why.contains("table-only: []"), "{why}");
}

#[test]
fn fwdtab_ac02_lookup_resolves_every_row_and_misses_aliases() {
    for name in dispatch_table::names() {
        assert!(dispatch_table::lookup(name).is_some(), "{name} has a lookup hit");
    }
    assert!(dispatch_table::lookup("host.secret_delete").is_none(), "an alias spelling is not a row");
    assert!(dispatch_table::lookup("billing.status").is_none(), "billing.* is not a host row");
}
