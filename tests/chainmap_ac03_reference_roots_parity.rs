//! PRD-mcphost-chain-near-miss-mapping-refused AC3 (P0) -- Given the
//! checker's reference-root set, When the parity test reads it and the keys of
//! `CONTEXT_KEYS`, Then the two are equal, and a scan of the new near-miss
//! function(s) and the `did_you_mean` rewrite function only (not
//! `StepRef::parse` or the `CONTEXT_KEYS` table) finds no root-name string
//! literal, the `steps`/`prev` distinction being read from the per-root
//! attribute on the table.

use mcphost::kinds::chain::{CONTEXT_KEYS, reference_roots};
use std::collections::BTreeSet;

fn chain_rs() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/kinds/chain.rs")).expect("read src/kinds/chain.rs")
}

/// The field names of `pub struct ContextKeys { ... }`, read from the source.
fn table_fields(text: &str) -> BTreeSet<String> {
    let start = text.find("pub struct ContextKeys {").expect("ContextKeys struct");
    let body = &text[start..];
    let body = &body[..body.find("\n}").expect("struct end")];
    body.lines()
        .filter_map(|l| l.trim().strip_prefix("pub "))
        .filter_map(|l| l.split_once(": &'static str"))
        .map(|(name, _)| name.to_string())
        .collect()
}

#[test]
fn the_checkers_roots_are_the_keys_of_context_keys() {
    let keys: BTreeSet<String> = [
        CONTEXT_KEYS.input,
        CONTEXT_KEYS.prev,
        CONTEXT_KEYS.steps,
        CONTEXT_KEYS.event,
        CONTEXT_KEYS.result,
        CONTEXT_KEYS.item,
        CONTEXT_KEYS.index,
    ]
    .map(String::from)
    .into();
    let roots: BTreeSet<String> = reference_roots().iter().map(|r| r.name.to_string()).collect();
    assert_eq!(roots, keys, "the checker's roots must be the table's keys");
    // The table's own fields: a key added to the struct and not listed above
    // (or not in the root set) fails here.
    assert_eq!(table_fields(&chain_rs()), keys, "ContextKeys fields vs the keys this test lists");
    assert_eq!(reference_roots().len(), keys.len(), "a root listed twice");
}

#[test]
fn steps_and_prev_are_told_apart_by_the_table_attribute() {
    let roots = reference_roots();
    let has_result: BTreeSet<&str> = roots.iter().filter(|r| r.has_result).map(|r| r.name).collect();
    let indexed: BTreeSet<&str> = roots.iter().filter(|r| r.indexed).map(|r| r.name).collect();
    assert_eq!(has_result, BTreeSet::from([CONTEXT_KEYS.prev, CONTEXT_KEYS.steps]));
    assert_eq!(indexed, BTreeSet::from([CONTEXT_KEYS.steps]));
}

/// The text of `fn <name>` up to its closing brace (comments stripped).
fn function_body(text: &str, name: &str) -> String {
    let at = ["\nfn ", "\npub fn ", "\nasync fn ", "\npub async fn "]
        .iter()
        .filter_map(|p| text.find(&format!("{p}{name}(")).or_else(|| text.find(&format!("{p}{name}<"))))
        .min()
        .unwrap_or_else(|| panic!("function {name} not found"));
    let rest = &text[at + 1..];
    let open = rest.find('{').expect("body");
    let mut depth = 0usize;
    let mut end = rest.len();
    for (i, c) in rest[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    rest[..end].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_near_miss_and_rewrite_functions_spell_no_root_name() {
    let text = chain_rs();
    let scanned = [
        "near_miss",
        "string_shape",
        "rewrite_reference",
        "accepted_rewrite",
        "root_named",
        "split_root",
        "well_formed_tail",
        "valid_token",
        "reference_roots",
    ];
    let mut offenders = Vec::new();
    for name in scanned {
        let body = function_body(&text, name);
        for root in reference_roots() {
            if body.contains(&format!("\"{}\"", root.name)) {
                offenders.push(format!("{name}: \"{}\"", root.name));
            }
        }
    }
    assert!(offenders.is_empty(), "root names spelled outside the table: {offenders:#?}");

    // The scan reaches the rewrite function's body, not an empty match.
    assert!(function_body(&text, "rewrite_reference").contains("CONTEXT_KEYS.result"));
    // StepRef::parse and the table are out of scope and may name `prev`.
    assert!(!scanned.contains(&"parse"));
}
