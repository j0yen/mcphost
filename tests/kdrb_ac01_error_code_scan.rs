//! PRD-mcphost-kind-docs-from-registry-b AC1: every error-code literal a
//! kind's source passes to `structured(..)` / `structured_with(..)` must be in
//! that kind's `KindDoc.error_codes`; a missing one fails naming code and kind.

use std::collections::BTreeSet;

use mcphost::kinds::production_registry;
use mcphost::plans::PlanCatalog;

/// `codes::NAME` -> value, read from the one file that spells them.
fn code_constants() -> Vec<(String, String)> {
    let src = include_str!("../src/kinds/contract.rs");
    let block = src.split("pub mod codes {").nth(1).and_then(|s| s.split("\n}").next()).unwrap_or("");
    block
        .lines()
        .filter_map(|l| {
            let l = l.trim().strip_prefix("pub const ")?;
            let (name, rest) = l.split_once(':')?;
            let value = rest.split('"').nth(1)?;
            Some((name.trim().to_string(), value.to_string()))
        })
        .collect()
}

/// Codes passed as the first argument of `structured(` / `structured_with(`.
fn scan_codes(source: &str) -> BTreeSet<String> {
    let consts = code_constants();
    let mut found = BTreeSet::new();
    for marker in ["structured(", "structured_with("] {
        for part in source.split(marker).skip(1) {
            let arg = part.trim_start().split([',', ')']).next().unwrap_or("").trim();
            if let Some(lit) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
                found.insert(lit.to_string());
            } else if let Some(name) = arg.strip_prefix("codes::") {
                let value = consts.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
                found.insert(value.unwrap_or_else(|| format!("codes::{name}")));
            }
        }
    }
    found
}

fn missing_codes(kind: &str, source: &str) -> Vec<String> {
    let doc = production_registry().get(kind).expect("registered").doc(&PlanCatalog::default_catalog());
    let table: BTreeSet<_> = doc.error_codes.into_iter().map(|e| e.code).collect();
    scan_codes(source).into_iter().filter(|c| !table.contains(c)).collect()
}

fn source_of(kind: &str) -> &'static str {
    match kind {
        "chain" => include_str!("../src/kinds/chain.rs"),
        "echo" => include_str!("../src/kinds/echo.rs"),
        "http" => include_str!("../src/kinds/http.rs"),
        "python" => include_str!("../src/kinds/python.rs"),
        "wasm" => include_str!("../src/kinds/wasm.rs"),
        other => panic!("kind `{other}` has no source mapping in this scan"),
    }
}

#[test]
fn kdrb_ac01_every_registered_kind_literal_is_in_its_error_table() {
    for kind in production_registry().names() {
        let missing = missing_codes(kind, source_of(kind));
        assert!(missing.is_empty(), "kind `{kind}`: error code(s) {missing:?} not in KindDoc.error_codes");
    }
}

#[test]
fn kdrb_ac01_new_literal_fails_naming_code_and_kind() {
    let injected = format!("{}\nfn x() {{ KindError::structured(\"upstream_slow\", \"slow\"); }}\n", source_of("http"));
    let missing = missing_codes("http", &injected);
    assert_eq!(missing, vec!["upstream_slow".to_string()]);
    // The assertion message the scan test emits names both.
    let msg = format!("kind `{}`: error code(s) {:?} not in KindDoc.error_codes", "http", missing);
    assert!(msg.contains("upstream_slow") && msg.contains("http"), "{msg}");
}
