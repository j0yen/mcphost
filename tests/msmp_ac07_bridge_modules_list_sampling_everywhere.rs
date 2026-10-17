//! PRD-mcphost-sampling-from-tools
//! AC7 (P0) — Given `BRIDGE_MODULES`, When `unknown_import_hint()` and
//! `docs/sandbox-api.md` are rendered, Then both list `mcphost.sampling` and
//! the `runner_script_registered_modules` test passes.

use mcphost::kinds::python::{
    BRIDGE_DOCS_END, BRIDGE_DOCS_START, BRIDGE_MODULES, render_bridge_docs_block, runner_script_registered_modules,
    unknown_import_hint,
};
use std::collections::BTreeSet;

#[test]
fn hint_docs_and_runner_script_all_list_the_sampling_module() {
    let hint = unknown_import_hint();
    assert!(hint.contains("mcphost.sampling"), "unknown_import_hint must list mcphost.sampling: {hint}");

    let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/sandbox-api.md")).expect("read docs");
    let start = docs.find(BRIDGE_DOCS_START).expect("docs/sandbox-api.md carries the bridge-modules start marker");
    let end = docs.find(BRIDGE_DOCS_END).expect("docs/sandbox-api.md carries the bridge-modules end marker") + BRIDGE_DOCS_END.len();
    let block = &docs[start..end];
    assert_eq!(block, render_bridge_docs_block(), "the docs block must be exactly what BRIDGE_MODULES renders");
    assert!(block.contains("`mcphost.sampling`"), "the docs block must list mcphost.sampling: {block}");

    // The independent half of the contract: the runner script registers exactly these modules.
    let registered: BTreeSet<String> = runner_script_registered_modules().into_iter().collect();
    assert!(registered.contains("sampling"), "the runner script must register mcphost.sampling: {registered:?}");
    let declared: BTreeSet<String> = BRIDGE_MODULES.iter().map(|m| m.name.to_string()).collect();
    assert_eq!(registered, declared, "BRIDGE_MODULES and the runner script's sys.modules lines must not drift");
}
