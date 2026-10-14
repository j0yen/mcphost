//! PRD-mcphost-tool-idempotent-declaration AC4 -- Given the registered
//! kinds, When the `idempotent_default` test runs, Then every kind has an
//! explicit implementation and `host.*` tools each have a `HOST_TOOL_HINTS`
//! row.

use crate::common;
use mcphost::handler::{HOST_TOOL_HINTS, canonical_control_plane_names};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::collections::HashSet;

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The trait method has no default body, and every `impl Kind for` in
/// `src/kinds/` writes its own `fn idempotent_default` -- so a new kind
/// cannot compile without choosing.
#[test]
fn trait_has_no_default_and_every_src_impl_is_explicit() {
    let trait_src = read("src/kinds/mod.rs");
    assert!(
        trait_src.contains("fn idempotent_default(&self, spec: &Value) -> bool;"),
        "Kind::idempotent_default must be a required method (no default body)"
    );
    let mut impls = 0;
    for file in ["echo", "http", "python", "chain", "wasm"] {
        let src = read(&format!("src/kinds/{file}.rs"));
        let count = src.matches("impl Kind for ").count();
        assert_eq!(count, 1, "{file}.rs: one Kind impl");
        assert!(src.contains("fn idempotent_default(&self"), "{file}.rs must implement idempotent_default explicitly");
        impls += count;
    }
    assert_eq!(impls, 5);
}

#[test]
fn every_registered_kind_answers_its_default() {
    let dir = common::TempDataDir::new();
    let kinds: KindRegistry = common::five_kinds_registry(&dir.0);
    let mut seen = HashSet::new();
    for kind in kinds.all() {
        seen.insert(kind.name());
        let spec: Value = match kind.name() {
            "http" => json!({"method": "GET", "url": "https://api.example.com/x"}),
            _ => json!({}),
        };
        let expected = kind.name() == "http";
        assert_eq!(kind.idempotent_default(&spec), expected, "kind {}", kind.name());
    }
    for kind in ["echo", "http", "python", "chain", "wasm"] {
        assert!(seen.contains(kind), "five_kinds_registry registers {kind}: {seen:?}");
    }
    let http = kinds.get("http").expect("http kind");
    let post = json!({"method": "POST", "url": "https://api.example.com/x"});
    assert!(!http.idempotent_default(&post));
    assert!(!http.idempotent_default(&json!({"method": "nonsense"})), "unparseable spec is not idempotent");
}

#[test]
fn every_control_plane_tool_has_a_hint_row_and_no_row_is_stale() {
    let dir = common::TempDataDir::new();
    let kinds = common::five_kinds_registry(&dir.0);
    let mut names = canonical_control_plane_names(&kinds);
    names.push("signup".to_string());
    let rows: HashSet<&str> = HOST_TOOL_HINTS.iter().map(|(n, _, _)| *n).collect();
    assert_eq!(rows.len(), HOST_TOOL_HINTS.len(), "HOST_TOOL_HINTS has a duplicate row");
    let missing: Vec<&String> = names.iter().filter(|n| !rows.contains(n.as_str())).collect();
    assert!(missing.is_empty(), "tools without a HOST_TOOL_HINTS row: {missing:?}");
    let live: HashSet<&str> = names.iter().map(String::as_str).collect();
    let stale: Vec<&&str> = rows.iter().filter(|n| !live.contains(**n)).collect();
    assert!(stale.is_empty(), "HOST_TOOL_HINTS rows for tools that do not exist: {stale:?}");
    // A read-only row must be idempotent.
    for (name, idempotent, read_only) in HOST_TOOL_HINTS {
        assert!(*idempotent || !*read_only, "{name}: read-only but not idempotent");
    }
    // The PRD's own examples.
    let hint = |n: &str| mcphost::handler::host_tool_hint(n).unwrap();
    assert!(hint("host.state.set").0);
    assert!(!hint("host.trigger.fire").0);
}
